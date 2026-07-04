//! sondir — the soil test before you build (Solana toolchain pre-flight & compatibility).
//!
//! `doctor` runs read-only pre-flight checks that turn deploy-time surprises
//! (SIMD-0431 extends, arch mismatches, stranded buffers, IDL init-vs-upgrade)
//! into actionable warnings BEFORE a transaction is sent.

mod checks;
mod facts;
mod fix;
mod mcp;
mod project;
mod report;
mod resolve;
mod rpc;
mod sweep;
mod verify;
mod watch;

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
    /// Override the embedded facts database with a facts.toml of your own.
    #[arg(long, global = true)]
    facts: Option<PathBuf>,
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
    /// Check upstream unlock triggers (litesvm Agave-4.1 wave, SIMD-0500, anchor wave).
    Watch {
        /// RPC for gate checks (else $SONDIR_RPC, else public devnet).
        #[arg(long)]
        url: Option<String>,
        /// Emit trigger statuses as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Find a mutually-compatible version set for a selection of ecosystem deps.
    Resolve {
        /// Ecosystem aliases or raw crate names (try: anchor litesvm magicblock). See --list.
        names: Vec<String>,
        /// List the known aliases.
        #[arg(long)]
        list: bool,
        /// Emit the resolution as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run as an MCP server (stdio) exposing doctor/resolve/watch to AI agents.
    Mcp,
    /// Facts database maintenance.
    Facts {
        #[command(subcommand)]
        cmd: FactsCommand,
    },
    /// Probe every ecosystem crate pair for conflicts the facts DB doesn't know yet.
    Sweep {
        /// Emit the sweep report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Apply facts-DB dependency-pin remedies to Cargo.toml (dry-run unless --write).
    Fix {
        /// Workspace root (containing Anchor.toml / Cargo.toml).
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Actually write the changes. Without this, fix only prints the plan.
        #[arg(long)]
        write: bool,
    },
}

#[derive(Subcommand)]
enum FactsCommand {
    /// Re-verify every facts entry against its live source (cargo probe, cluster RPC).
    Verify {
        /// RPC for gate checks (else $SONDIR_RPC, else public devnet).
        #[arg(long)]
        url: Option<String>,
        /// Emit statuses as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(err) = facts::load(cli.facts.as_deref()) {
        eprintln!("error: {err:#}");
        return ExitCode::from(2);
    }
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
        Command::Watch { url, json } => {
            let rpc_url = url
                .or_else(|| std::env::var("SONDIR_RPC").ok())
                .unwrap_or_else(|| "https://api.devnet.solana.com".into());
            watch::run(&rpc_url, json)
        }
        Command::Resolve { names, list, json } => {
            if list || names.is_empty() {
                resolve::list_aliases();
                return Ok(0);
            }
            resolve::run(&names, json)
        }
        Command::Mcp => mcp::serve(),
        Command::Facts {
            cmd: FactsCommand::Verify { url, json },
        } => {
            let rpc_url = url
                .or_else(|| std::env::var("SONDIR_RPC").ok())
                .unwrap_or_else(|| "https://api.devnet.solana.com".into());
            verify::run(&rpc_url, json)
        }
        Command::Sweep { json } => sweep::run(json),
        Command::Fix { path, write } => {
            let project = Project::load(&path)?;
            fix::run(&project, write)
        }
    }
}

fn doctor(path: &std::path::Path, url: Option<&str>, json: bool, offline: bool) -> Result<i32> {
    let report = run_doctor(path, url, offline)?;
    report.print(json)
}

/// Run every pre-flight check and return the report without printing (shared by
/// the CLI `doctor` command and the MCP server).
pub fn run_doctor(path: &std::path::Path, url: Option<&str>, offline: bool) -> Result<Report> {
    let project = Project::load(path)?;
    let mut report = Report::default();

    checks::toolchain(&mut report, &project);
    checks::known_conflicts(&mut report, &project);
    checks::resolve_probe(&mut report, &project, offline);
    checks::anchor_test_footgun(&mut report, &project);

    let built = project.artifacts();
    // Purely local (keypair files vs Anchor.toml) — must run in offline mode too.
    checks::keypair_drift(&mut report, &project);

    if offline {
        report.info("offline", "offline mode", "cluster checks skipped");
        // Arch-vs-litesvm still works offline; cluster gates default to inactive.
        let gate = checks::GateStatus {
            sbpf_v3: true,
            simd_0431: false,
            simd_0500: false,
        };
        checks::artifacts(&mut report, &project, &built, &gate);
        return Ok(report);
    }

    let rpc = RpcClient::new(project.rpc_url(url));
    report.info(
        "rpc",
        format!("cluster RPC: {}", rpc.url()),
        "override with --url or $SONDIR_RPC",
    );

    match checks::gates(&mut report, &rpc) {
        Ok(gate) => {
            checks::artifacts(&mut report, &project, &built, &gate);
            checks::upgrade_preflight(&mut report, &rpc, &project, &built, &gate);
            checks::idl_drift(&mut report, &rpc, &project, &built);
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

    Ok(report)
}
