//! `sondir facts verify` — re-verify every facts.toml entry against its live
//! source, so the knowledge base can't rot silently.
//!
//! A conflict entry claims "these deps cannot resolve together"; verifying it
//! means running cargo's resolver on exactly that selection and EXPECTING
//! failure. The day upstream fixes it (e.g. litesvm ships its Agave-4.1 wave),
//! the probe resolves, the entry flips to STALE, and exit code 4 tells CI to
//! alert. Gate entries are checked against the cluster; runtime arch claims
//! can only be re-verified by a canary run, so they stay evidence-based.

use anyhow::Result;
use serde::Serialize;

use crate::facts;
use crate::resolve::{probe, ProbeResult};
use crate::rpc::RpcClient;

#[derive(Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// The claim still holds against its live source.
    Verified,
    /// The live source contradicts the claim — update or retire the entry.
    Stale,
    /// Not auto-verifiable; rests on recorded canary evidence.
    Evidence,
    /// Verification could not run (network/cargo failure, malformed probe).
    Unchecked,
}

#[derive(Serialize)]
pub struct FactStatus {
    pub kind: &'static str,
    pub id: String,
    pub status: Status,
    pub detail: String,
}

pub fn collect(rpc_url: &str) -> Vec<FactStatus> {
    let mut statuses = Vec::new();

    for conflict in facts::conflicts() {
        statuses.push(verify_conflict(conflict));
    }

    let rpc = RpcClient::new(rpc_url);
    for gate in facts::gates() {
        let (status, detail) = match rpc.feature_active(&gate.address) {
            Ok(Some(active)) => (
                Status::Verified,
                format!(
                    "{}: feature account present on {rpc_url} ({})",
                    gate.simd,
                    if active { "ACTIVE" } else { "pending activation" }
                ),
            ),
            Ok(None) => (
                Status::Unchecked,
                format!(
                    "{}: account absent on this cluster (inactive/not scheduled) — address itself not verifiable here",
                    gate.simd
                ),
            ),
            Err(err) => (Status::Unchecked, format!("RPC failed: {err:#}")),
        };
        statuses.push(FactStatus {
            kind: "gate",
            id: gate.simd.clone(),
            status,
            detail,
        });
    }

    for runtime in facts::litesvm_runtimes() {
        statuses.push(FactStatus {
            kind: "litesvm-runtime",
            id: format!("litesvm {}x", runtime.prefix),
            status: Status::Evidence,
            detail: format!(
                "arch claim {:?} needs a VM execution to re-verify (canary evidence): {}",
                runtime.arch_ok, runtime.note
            ),
        });
    }

    statuses
}

fn verify_conflict(conflict: &facts::KnownConflict) -> FactStatus {
    if conflict.probe.is_empty() {
        return FactStatus {
            kind: "conflict",
            id: conflict.id.clone(),
            status: Status::Evidence,
            detail: "no machine-checkable probe recorded; rests on canary evidence".into(),
        };
    }
    let deps: Vec<(String, String, Vec<String>)> = conflict
        .probe
        .iter()
        .filter_map(|spec| facts::parse_probe(spec))
        .collect();
    if deps.len() != conflict.probe.len() {
        return FactStatus {
            kind: "conflict",
            id: conflict.id.clone(),
            status: Status::Unchecked,
            detail: format!("malformed probe spec in {:?}", conflict.probe),
        };
    }
    match probe(&deps, &conflict.id) {
        Ok(ProbeResult::Conflicts(stderr)) => {
            let line = stderr
                .lines()
                .find(|l| l.contains("failed to select"))
                .unwrap_or("cargo resolution failed")
                .trim()
                .to_owned();
            FactStatus {
                kind: "conflict",
                id: conflict.id.clone(),
                status: Status::Verified,
                detail: format!("still unresolvable — {line}"),
            }
        }
        Ok(ProbeResult::Resolves(locked)) => {
            let resolved: Vec<String> = deps
                .iter()
                .filter_map(|(name, _, _)| locked.get(name).map(|v| format!("{name} {v}")))
                .collect();
            FactStatus {
                kind: "conflict",
                id: conflict.id.clone(),
                status: Status::Stale,
                detail: format!(
                    "now RESOLVES ({}) — upstream fixed it; update or retire this entry",
                    resolved.join(", ")
                ),
            }
        }
        Err(err) => FactStatus {
            kind: "conflict",
            id: conflict.id.clone(),
            status: Status::Unchecked,
            detail: format!("probe could not run: {err:#}"),
        },
    }
}

pub fn run(rpc_url: &str, json: bool) -> Result<i32> {
    let statuses = collect(rpc_url);
    let stale = statuses
        .iter()
        .filter(|s| s.status == Status::Stale)
        .count();
    if json {
        println!("{}", serde_json::to_string_pretty(&statuses)?);
    } else {
        for entry in &statuses {
            let glyph = match entry.status {
                Status::Verified => "✓ VERIFIED ",
                Status::Stale => "✗ STALE    ",
                Status::Evidence => "i evidence ",
                Status::Unchecked => "? unchecked",
            };
            println!("{glyph} [{}:{}]", entry.kind, entry.id);
            println!("            {}\n", entry.detail);
        }
        println!("{} entries · {} stale", statuses.len(), stale);
    }
    // Exit 4 on stale facts (3 already means "watch trigger fired").
    Ok(if stale > 0 { 4 } else { 0 })
}
