//! sondir — the soil test before you build (Solana toolchain pre-flight & compatibility).
//!
//! `doctor` runs read-only pre-flight checks that turn deploy-time surprises
//! (SIMD-0431 extends, arch mismatches, stranded buffers, IDL init-vs-upgrade)
//! into actionable warnings BEFORE a transaction is sent.

mod checks;
mod facts;
mod project;
mod report;
mod rpc;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::project::Project;
use crate::report::Report;
use crate::rpc::RpcClient;

#[derive(Parser)]
#[command(name = "sondir", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Pre-flight checks for an Anchor workspace (read-only: local files + RPC reads).
    Doctor {
        /// Workspace root (containing Anchor.toml).
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// RPC URL override (else $SONDIR_RPC, else Anchor.toml provider.cluster).
        #[arg(long)]
        url: Option<String>,
        /// Emit findings as JSON (for agents / CI).
        #[arg(long)]
        json: bool,
        /// Skip all RPC calls (offline mode: local checks only).
        #[arg(long)]
        offline: bool,
    },
    /// (roadmap) Resolve a compatible version set for a selection of ecosystem deps.
    Resolve,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Doctor {
            path,
            url,
            json,
            offline,
        } => doctor(&path, url.as_deref(), json, offline),
        Command::Resolve => {
            println!(
                "resolve is on the roadmap: multi-select ecosystem deps -> synthetic manifest -> cargo resolver -> explained version set.\nFor now see the curated facts in this binary (doctor) and the raflux toolchain matrix doc."
            );
            Ok(0)
        }
    }
}

fn doctor(path: &std::path::Path, url: Option<&str>, json: bool, offline: bool) -> Result<i32> {
    let project = Project::load(path)?;
    let mut report = Report::default();

    checks::toolchain(&mut report, &project);
    checks::known_conflicts(&mut report, &project);
    checks::anchor_test_footgun(&mut report, &project);
    checks::idl_rule(&mut report);

    let built = project.artifacts();

    if offline {
        report.info("offline", "offline mode", "cluster checks skipped");
        // Arch-vs-litesvm still works offline; cluster gates default to inactive.
        let gate = checks::GateStatus {
            sbpf_v3: true,
            simd_0431: false,
            simd_0500: false,
        };
        checks::artifacts(&mut report, &project, &built, &gate);
        return report.print(json);
    }

    let rpc = RpcClient::new(project.rpc_url(url));
    report.info(
        "rpc",
        format!("cluster RPC: {}", rpc.url()),
        "override with --url or $SONDIR_RPC",
    );

    checks::keypair_drift(&mut report, &built);

    match checks::gates(&mut report, &rpc) {
        Ok(gate) => {
            checks::artifacts(&mut report, &project, &built, &gate);
            checks::upgrade_preflight(&mut report, &rpc, &project, &built, &gate);
            checks::stranded_buffers(&mut report, &rpc, &project);
            checks::balance(&mut report, &rpc, &project, &built);
        }
        Err(err) => {
            report.warn(
                "rpc",
                "cluster unreachable — on-chain checks skipped",
                format!("{err:#}"),
                Some(
                    "re-run with --url <working RPC> (public devnet rate-limits aggressively)"
                        .into(),
                ),
            );
        }
    }

    report.print(json)
}
