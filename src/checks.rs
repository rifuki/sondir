//! Doctor checks. Each converts a class of deploy-time surprise into a
//! pre-flight finding with a concrete fix.

use std::process::Command;
use std::str::FromStr;

use anyhow::Result;
use solana_pubkey::Pubkey;

use crate::facts;
use crate::project::{keypair_pubkey, tool_version, Artifact, Project};
use crate::report::Report;
use crate::rpc::{lamports_to_sol, RpcClient};

pub struct GateStatus {
    pub sbpf_v3: bool,
    pub simd_0431: bool,
    pub simd_0500: bool,
}

/// Anchor CLI vs anchor-lang crate pairing + toolchain visibility.
pub fn toolchain(report: &mut Report, project: &Project) {
    let anchor_cli = tool_version("anchor");
    let solana_cli = tool_version("solana");
    let anchor_lang = project.locked.get("anchor-lang").cloned();

    match (&anchor_cli, &anchor_lang) {
        (Some(cli), Some(lang)) => {
            let cli_version = cli.split_whitespace().last().unwrap_or_default();
            if cli_version == lang {
                let pin = project.anchor.toolchain.anchor_version.as_deref();
                let pin_note = match pin {
                    Some(pinned) if pinned == lang => "pinned via [toolchain] anchor_version".into(),
                    Some(pinned) => format!("NOTE: [toolchain] anchor_version = \"{pinned}\" disagrees — update the pin"),
                    None => "consider pinning [toolchain] anchor_version in Anchor.toml".into(),
                };
                report.ok(
                    "toolchain-anchor",
                    format!("anchor CLI {cli_version} == anchor-lang {lang}"),
                    pin_note,
                );
            } else {
                report.warn(
                    "toolchain-anchor",
                    format!("anchor CLI {cli_version} != anchor-lang {lang}"),
                    "every build will print a mismatch warning; subtle IDL/codegen drift is possible",
                    Some(format!(
                        "either `avm use {lang}` or bump anchor-lang/anchor-spl to {cli_version} (pin [toolchain] anchor_version)"
                    )),
                );
            }
        }
        (None, _) => report.warn(
            "toolchain-anchor",
            "anchor CLI not found on PATH",
            "cannot verify CLI/crate pairing",
            None,
        ),
        (_, None) => report.info(
            "toolchain-anchor",
            "anchor-lang not in Cargo.lock",
            "not an Anchor workspace (or lockfile missing)",
        ),
    }

    if let Some(solana) = solana_cli {
        report.info(
            "toolchain-solana",
            solana,
            "CLI binary — never constrains crate versions",
        );
    }
}

/// Does the workspace even resolve? The lockfile can look healthy while the
/// manifest graph is unresolvable (`cargo add` writes the dep, then fails to
/// re-lock) — so probe cargo itself. Found by canary c05.
pub fn resolve_probe(report: &mut Report, project: &Project, offline: bool) {
    let mut args = vec!["metadata", "--format-version", "1"];
    if offline {
        // Honor doctor's no-network contract; cargo may otherwise hit the index.
        args.push("--offline");
    }
    let output = Command::new("cargo")
        .args(&args)
        .current_dir(&project.root)
        .output();
    match output {
        Ok(out) if out.status.success() => report.ok(
            "resolve",
            "dependency graph resolves",
            "cargo metadata succeeded",
        ),
        Ok(out) if offline && String::from_utf8_lossy(&out.stderr).contains("--offline") => {
            report.info(
                "resolve",
                "resolve probe inconclusive offline",
                "cargo needs the network to (re-)resolve this workspace; re-run without --offline",
            );
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let mut summary: String = stderr
                .lines()
                .filter(|line| {
                    line.contains("failed to select")
                        || line.contains("required by")
                        || line.contains("previously selected")
                        || line.contains("which satisfies")
                })
                .take(5)
                .collect::<Vec<_>>()
                .join("\n");
            if summary.is_empty() {
                summary = stderr.lines().take(3).collect::<Vec<_>>().join("\n");
            }
            report.fail(
                "resolve",
                "dependency graph does NOT resolve",
                summary,
                Some(
                    "see the dep-conflict finding for known causes; full chain: `cargo metadata`"
                        .into(),
                ),
            );
        }
        Err(err) => report.warn(
            "resolve",
            "cargo not found — resolve probe skipped",
            format!("{err}"),
            None,
        ),
    }
}

/// Known unresolvable dependency pairs, straight from the facts DB. Reads BOTH
/// the lockfile and the declared manifests (the lock lies after a failed
/// resolve — canary c05).
pub fn known_conflicts(report: &mut Report, project: &Project) {
    let declared = project.declared_deps();
    let has_magicblock_vrf = project.locked.contains_key("ephemeral-vrf-sdk")
        || declared.contains_key("ephemeral-vrf-sdk")
        || declared.contains_key("ephemeral-rollups-sdk");

    // The lock and the manifest can disagree (stale lock after a failed
    // resolve, canary c05: lock said 0.10.0 while the manifest declared
    // 0.13.1) — treat EITHER source matching as a hit.
    let is_013 = |v: &str| v.trim_start_matches(['^', '=', '~']).starts_with("0.13");
    let locked_litesvm = project.locked.get("litesvm").cloned();
    let declared_litesvm = declared.get("litesvm").cloned();
    let litesvm_013 = locked_litesvm.as_deref().is_some_and(is_013)
        || declared_litesvm.as_deref().is_some_and(is_013);

    let mut conflict_hit = false;
    if litesvm_013 && has_magicblock_vrf {
        if let Some(conflict) = facts::conflict("litesvm-magicblock") {
            report.fail(
                "dep-conflict",
                format!("{} × {}", conflict.a, conflict.b),
                conflict.why.clone(),
                Some(conflict.fix.clone()),
            );
            conflict_hit = true;
        }
    }

    // litesvm 0.13 × instructions-sysvar >=3.0.1 (canary c06). `=3.0.0` is the
    // one escape hatch, so only that exact pin is exempt.
    if litesvm_013 {
        if let Some(sysvar) = declared.get("solana-instructions-sysvar") {
            if sysvar != "=3.0.0" {
                if let Some(conflict) = facts::conflict("litesvm-instructions-sysvar") {
                    report.fail(
                        "dep-conflict",
                        format!("{} × {}", conflict.a, conflict.b),
                        conflict.why.clone(),
                        Some(conflict.fix.clone()),
                    );
                    conflict_hit = true;
                }
            }
        }
    }

    // Legacy solana-program 1.x inside a modern workspace (canary c19).
    let legacy_solana_program = declared
        .get("solana-program")
        .or_else(|| project.locked.get("solana-program"))
        .is_some_and(|v| v.trim_start_matches(['^', '=', '~']).starts_with("1."));
    if legacy_solana_program {
        if let Some(conflict) = facts::conflict("legacy-solana-program") {
            report.fail(
                "dep-conflict",
                format!("{} × {}", conflict.a, conflict.b),
                conflict.why.clone(),
                Some(conflict.fix.clone()),
            );
            conflict_hit = true;
        }
    }

    if !conflict_hit {
        if let Some(version) = locked_litesvm.or(declared_litesvm) {
            let version = version.trim_start_matches(['^', '=', '~']);
            if let Some(runtime) = facts::litesvm_runtime(version) {
                report.info(
                    "litesvm-runtime",
                    format!("litesvm {version}"),
                    runtime.note.clone(),
                );
            }
        }
    }
}

/// Cluster feature gates that change deploy semantics.
///
/// One flaky gate read no longer aborts everything (audit follow-up): the
/// failed gate falls back to a CONSERVATIVE assumption (v3 enabled, min-extend
/// enforced, SIMD-0500 not yet active) with a warn. Only when EVERY read fails
/// do we bail so main can skip on-chain checks entirely.
pub fn gates(report: &mut Report, rpc: &RpcClient) -> Result<GateStatus> {
    // Conservative defaults: assuming SIMD-0431 active only over-recommends a
    // harmless manual extend; assuming SBPFv3 enabled matches every cluster today.
    let mut status = GateStatus {
        sbpf_v3: true,
        simd_0431: true,
        simd_0500: false,
    };
    let mut failures = 0usize;
    for gate in facts::gates() {
        let active = match rpc.feature_active(&gate.address) {
            Ok(state) => state.unwrap_or(false),
            Err(err) => {
                failures += 1;
                report.warn(
                    "gate",
                    format!("{} status unknown (RPC read failed)", gate.simd),
                    format!("{err:#}\nassuming the conservative default"),
                    None,
                );
                continue;
            }
        };
        match gate.simd.as_str() {
            "SIMD-0178/0189/0377" => status.sbpf_v3 = active,
            "SIMD-0431" => status.simd_0431 = active,
            "SIMD-0500" => status.simd_0500 = active,
            _ => {}
        }
        let state = if active { "ACTIVE" } else { "inactive" };
        report.info(
            "gate",
            format!("{} {} — {state}", gate.simd, gate.name),
            if active {
                gate.consequence.clone()
            } else {
                "no effect yet".to_owned()
            },
        );
    }
    if failures == facts::gates().len() {
        anyhow::bail!("all feature-gate reads failed — RPC unreachable");
    }
    Ok(status)
}

/// SBPF arch of each built .so vs (a) the target cluster and (b) the local
/// litesvm test runtime.
pub fn artifacts(report: &mut Report, project: &Project, built: &[Artifact], gate: &GateStatus) {
    if built.is_empty() {
        report.info(
            "artifacts",
            "no built .so in target/deploy",
            "run `anchor build` first for artifact checks",
        );
        return;
    }

    let litesvm_runtime = project.dep_version("litesvm").and_then(|version| {
        facts::litesvm_runtime(&version).map(|runtime| (version.clone(), runtime))
    });

    for artifact in built {
        let flag = artifact.sbpf_flag;
        if flag == u32::MAX {
            report.fail(
                "arch-cluster",
                format!("{}.so is not a valid ELF (or is truncated)", artifact.name),
                format!("{} bytes · {}", artifact.so_len, artifact.so_path.display()),
                Some("rebuild the artifact (`anchor build`)".into()),
            );
            continue;
        }
        match facts::arch_deployable(flag, gate.sbpf_v3, gate.simd_0500) {
            Ok(()) => report.ok(
                "arch-cluster",
                format!(
                    "{}.so arch v{flag} — deployable on target cluster",
                    artifact.name
                ),
                format!("{} bytes · {}", artifact.so_len, artifact.so_path.display()),
            ),
            Err(why) => report.fail(
                "arch-cluster",
                format!("{}.so arch v{flag} — NOT deployable", artifact.name),
                why,
                None,
            ),
        }

        if let Some((version, runtime)) = &litesvm_runtime {
            if !runtime.arch_ok.contains(&flag) {
                let wanted = runtime
                    .arch_ok
                    .iter()
                    .map(|v| format!("v{v}"))
                    .collect::<Vec<_>>()
                    .join("/");
                report.warn(
                    "arch-litesvm",
                    format!(
                        "{}.so arch v{flag} will NOT run under litesvm {version} tests",
                        artifact.name
                    ),
                    format!("this litesvm runtime executes {wanted} only — expect \"Access violation\" failures"),
                    Some(format!(
                        "rebuild the test artifact with `cargo build-sbf --arch {}` before `cargo test`",
                        runtime.arch_ok.first().map(|v| format!("v{v}")).unwrap_or_default()
                    )),
                );
            }
        }
    }
}

/// `anchor deploy` targets the address in `target/deploy/<name>-keypair.json`,
/// NOT the id in Anchor.toml or `declare_id!`. When they disagree, deploys land
/// on a different (possibly fresh) program id while the code self-identifies as
/// another — the classic silent-wrong-address incident.
pub fn keypair_drift(report: &mut Report, built: &[Artifact]) {
    for artifact in built {
        let Some(config_id) = &artifact.program_id else {
            continue;
        };
        let keypair_path = artifact
            .so_path
            .with_file_name(format!("{}-keypair.json", artifact.name.replace('-', "_")));
        let Ok(keypair_id) = keypair_pubkey(&keypair_path) else {
            continue;
        };
        if &keypair_id == config_id {
            report.ok(
                "keypair-drift",
                format!("{}: deploy keypair matches Anchor.toml", artifact.name),
                config_id.clone(),
            );
        } else {
            report.fail(
                "keypair-drift",
                format!("{}: deploy keypair != Anchor.toml program id", artifact.name),
                format!(
                    "anchor deploy will target {keypair_id}\nAnchor.toml declares    {config_id}\n(declare_id! may be a third value — anchor build warns about that one)"
                ),
                Some(
                    "throwaway project: run `anchor keys sync` AND update [programs.*] in Anchor.toml.\nlive project: remove/move the stray keypair and upgrade via `solana program deploy --program-id <id>` with the real authority".into(),
                ),
            );
        }
    }
}

/// The SIMD-0431 trap: an upgrade whose binary grew by 1..10239 bytes fails
/// mid-flight and strands the buffer. Detectable entirely pre-flight.
pub fn upgrade_preflight(
    report: &mut Report,
    rpc: &RpcClient,
    project: &Project,
    built: &[Artifact],
    gate: &GateStatus,
) {
    let Ok(loader) = Pubkey::from_str(facts::UPGRADEABLE_LOADER) else {
        return;
    };
    let wallet = wallet_pubkey(project);

    for artifact in built {
        let Some(program_id) = &artifact.program_id else {
            continue;
        };
        let Ok(program_key) = Pubkey::from_str(program_id) else {
            report.warn(
                "upgrade-preflight",
                format!("{}: invalid program id {program_id}", artifact.name),
                "Anchor.toml entry is not a valid pubkey",
                None,
            );
            continue;
        };
        let (programdata, _) = Pubkey::find_program_address(&[program_key.as_ref()], &loader);
        // One flaky RPC read must not kill the rest of the report.
        let programdata_account = match rpc.account(&programdata.to_string()) {
            Ok(account) => account,
            Err(err) => {
                report.warn(
                    "upgrade-preflight",
                    format!("{}: RPC read failed — check skipped", artifact.name),
                    format!("{err:#}"),
                    None,
                );
                continue;
            }
        };
        if let Some(account) = &programdata_account {
            if account.owner != facts::UPGRADEABLE_LOADER {
                report.warn(
                    "upgrade-preflight",
                    format!(
                        "{}: programdata owner is not the upgradeable loader",
                        artifact.name
                    ),
                    format!("owner: {}", account.owner),
                    None,
                );
                continue;
            }
        }
        let Some(account) = programdata_account else {
            let detail = match rpc.min_rent(artifact.so_len + facts::PROGRAMDATA_METADATA_LEN) {
                Ok(rent) => format!(
                    "expect ~{:.2} SOL locked as programdata rent, ~{:.2} SOL peak during deploy (buffer + programdata coexist until the buffer refunds)",
                    lamports_to_sol(rent),
                    lamports_to_sol(rent * 2),
                ),
                Err(_) => "rent estimate unavailable (RPC read failed)".into(),
            };
            report.info(
                "upgrade-preflight",
                format!(
                    "{}: fresh deploy (no programdata on cluster)",
                    artifact.name
                ),
                detail,
            );
            continue;
        };

        let capacity = (account.data.len() as u64).saturating_sub(facts::PROGRAMDATA_METADATA_LEN);
        if artifact.so_len <= capacity {
            report.ok(
                "upgrade-preflight",
                format!("{}: fits existing programdata", artifact.name),
                format!(
                    "binary {} bytes <= capacity {capacity} bytes — upgrade needs no extend",
                    artifact.so_len
                ),
            );
        } else {
            let delta = artifact.so_len - capacity;
            if gate.simd_0431 && delta < facts::MIN_EXTEND_BYTES {
                report.fail(
                    "simd0431-extend",
                    format!("{}: upgrade WILL fail — binary grew +{delta} bytes", artifact.name),
                    "SIMD-0431 is active: ExtendProgram requires >= 10240 bytes; anchor's auto-extend requests the exact delta and gets rejected AFTER writing the buffer (stranding its rent)",
                    Some(format!(
                        "solana program extend {program_id} {} -u {} -k {}\nthen re-run the deploy (the buffer auto-resumes)",
                        facts::MIN_EXTEND_BYTES,
                        rpc.url(),
                        project.anchor.provider.wallet,
                    )),
                );
            } else if gate.simd_0431 {
                // Anchor 1.1.2's auto-extend has been observed under-requesting
                // (asked +120 when the true capacity delta was ~24K) — pre-extend
                // manually rather than trusting it.
                report.warn(
                    "simd0431-extend",
                    format!("{}: binary grew +{delta} bytes — pre-extend before upgrading", artifact.name),
                    "SIMD-0431 is active and anchor's auto-extend can under-request the extension, failing AFTER the buffer is written (stranding its rent)",
                    Some(format!(
                        "solana program extend {program_id} {} -u {} -k {}",
                        delta.max(facts::MIN_EXTEND_BYTES),
                        rpc.url(),
                        project.anchor.provider.wallet,
                    )),
                );
            } else {
                report.info(
                    "upgrade-preflight",
                    format!("{}: binary grew +{delta} bytes", artifact.name),
                    "auto-extend should handle this on upgrade (SIMD-0431 not active here)",
                );
            }
        }

        // Deployed drift: is what's on chain the same bytes as the local build?
        // Programdata = 45-byte metadata + program bytes, zero-padded by extends;
        // compare with trailing zeros stripped on both sides (solana-verify's rule).
        if let Some(on_chain) = account.data.get(facts::PROGRAMDATA_METADATA_LEN as usize..) {
            if let Ok(local) = std::fs::read(&artifact.so_path) {
                if strip_trailing_zeros(on_chain) == strip_trailing_zeros(&local) {
                    report.ok(
                        "deployed-drift",
                        format!("{}: on-chain program matches local build", artifact.name),
                        "deployed bytes == target/deploy .so (trailing zeros ignored)",
                    );
                } else {
                    report.info(
                        "deployed-drift",
                        format!(
                            "{}: on-chain program DIFFERS from local build",
                            artifact.name
                        ),
                        format!(
                            "deployed {} bytes vs local {} bytes (both trimmed) — an upgrade (or a bare `anchor test` on this cluster) would replace the deployed program",
                            strip_trailing_zeros(on_chain).len(),
                            strip_trailing_zeros(&local).len(),
                        ),
                    );
                }
            }
        }

        // Upgrade authority: bincode ProgramData = tag(4) + slot(8) + Option<Pubkey>(1+32).
        if let (Some(wallet), Some(1)) = (&wallet, account.data.get(12).copied()) {
            if let Some(authority_bytes) = account.data.get(13..45) {
                let authority = Pubkey::try_from(authority_bytes)
                    .map(|k| k.to_string())
                    .unwrap_or_default();
                if &authority != wallet {
                    report.warn(
                        "upgrade-authority",
                        format!("{}: wallet is not the upgrade authority", artifact.name),
                        format!("authority on-chain: {authority}\nconfigured wallet:   {wallet}"),
                        Some("upgrade with the real authority keypair, or update Anchor.toml provider.wallet".into()),
                    );
                }
            }
        }
    }
}

/// Buffers left behind by interrupted deploys hold real rent.
pub fn stranded_buffers(report: &mut Report, rpc: &RpcClient, project: &Project) {
    for keypair_path in project.stranded_buffer_keypairs() {
        let Ok(buffer) = keypair_pubkey(&keypair_path) else {
            continue;
        };
        // An RPC error must not read as "0 SOL, nothing stranded" (audit pass 1).
        let lamports = match rpc.balance(&buffer) {
            Ok(lamports) => lamports,
            Err(err) => {
                report.warn(
                    "stranded-buffer",
                    format!("could not check buffer {buffer}"),
                    format!("{err:#}"),
                    None,
                );
                continue;
            }
        };
        if lamports > 0 {
            report.warn(
                "stranded-buffer",
                format!(
                    "stranded upgrade buffer holds {:.3} SOL",
                    lamports_to_sol(lamports)
                ),
                format!("{} (keypair {})", buffer, keypair_path.display()),
                Some(format!(
                    "re-run the matching deploy to consume it, or reclaim: solana program close {buffer} -u {} -k {}",
                    rpc.url(),
                    project.anchor.provider.wallet,
                )),
            );
        }
    }
}

/// Enough SOL for the largest pending buffer?
pub fn balance(report: &mut Report, rpc: &RpcClient, project: &Project, built: &[Artifact]) {
    let Some(wallet) = wallet_pubkey(project) else {
        if !project.anchor.provider.wallet.trim().is_empty() {
            report.warn(
                "balance",
                "provider wallet could not be read — balance/authority checks skipped",
                format!(
                    "Anchor.toml provider.wallet = \"{}\"",
                    project.anchor.provider.wallet
                ),
                Some("check the path (and that solana-keygen is on PATH)".into()),
            );
        }
        return;
    };
    let Ok(lamports) = rpc.balance(&wallet) else {
        return;
    };

    let largest = built.iter().map(|a| a.so_len).max().unwrap_or(0);
    if largest == 0 {
        return;
    }
    let buffer_rent = rpc
        .min_rent(largest + facts::BUFFER_METADATA_LEN)
        .unwrap_or_default();
    if lamports < buffer_rent {
        report.warn(
            "balance",
            format!(
                "wallet {:.3} SOL < {:.3} SOL needed for the largest upgrade buffer",
                lamports_to_sol(lamports),
                lamports_to_sol(buffer_rent)
            ),
            format!("wallet {wallet}; buffers refund after a successful upgrade, but the rent must be available up front"),
            Some("top up the wallet before deploying".into()),
        );
    } else {
        report.ok(
            "balance",
            format!("wallet holds {:.3} SOL", lamports_to_sol(lamports)),
            format!(
                "covers the largest upgrade buffer (~{:.3} SOL, refunded)",
                lamports_to_sol(buffer_rent)
            ),
        );
    }
}

/// `anchor test` deploys to provider.cluster before running tests.
pub fn anchor_test_footgun(report: &mut Report, project: &Project) {
    let cluster = &project.anchor.provider.cluster;
    if cluster.is_empty() || cluster == "localnet" || cluster == "localhost" {
        return;
    }
    let script = project
        .anchor
        .scripts
        .get("test")
        .map(String::as_str)
        .unwrap_or("<none>");
    report.warn(
        "anchor-test-footgun",
        format!("bare `anchor test` will DEPLOY to {cluster} first"),
        format!(
            "with a non-local provider.cluster, anchor test builds (default arch) and upgrades your live programs before running tests\n[scripts] test = \"{script}\""
        ),
        Some("always run `anchor test --skip-deploy` (or wire tests through [scripts] with cargo test directly)".into()),
    );
}

/// Compare the canonical on-chain IDL (Program Metadata account) with the
/// local `target/idl/*.json`. Replaces the old static `idl-rule` note with a
/// real check: absent → init; drifted → upgrade; corrupt → close + init. The
/// wrong choice fails with an opaque "transaction plan failed" AFTER fees.
pub fn idl_drift(report: &mut Report, rpc: &RpcClient, project: &Project, built: &[Artifact]) {
    let Ok(pmp) = Pubkey::from_str(facts::PROGRAM_METADATA_PROGRAM) else {
        return;
    };
    for artifact in built {
        let Some(program_id) = &artifact.program_id else {
            continue;
        };
        let Ok(program_key) = Pubkey::from_str(program_id) else {
            continue; // upgrade_preflight already reported the bad id
        };
        let idl_path = project
            .root
            .join("target/idl")
            .join(format!("{}.json", artifact.name.replace('-', "_")));
        let local: Option<serde_json::Value> = std::fs::read(&idl_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());

        let (metadata_pda, _) =
            Pubkey::find_program_address(&[program_key.as_ref(), &facts::IDL_SEED_PADDED], &pmp);
        let account = match rpc.account(&metadata_pda.to_string()) {
            Ok(account) => account,
            Err(err) => {
                report.warn(
                    "idl-drift",
                    format!("{}: RPC read failed — IDL check skipped", artifact.name),
                    format!("{err:#}"),
                    None,
                );
                continue;
            }
        };

        let Some(account) = account else {
            if local.is_some() {
                report.info(
                    "idl-drift",
                    format!("{}: no canonical on-chain IDL", artifact.name),
                    format!("metadata account {metadata_pda} does not exist — explorers/clients can't discover the IDL. First write must be an init: `anchor idl init {program_id} --filepath {}` (an `idl upgrade` here fails)", idl_path.display()),
                );
            }
            continue;
        };

        match on_chain_idl_json(&account.data) {
            OnChainIdl::Json(on_chain) => match &local {
                Some(local_idl) if *local_idl == on_chain => report.ok(
                    "idl-drift",
                    format!("{}: on-chain IDL matches target/idl", artifact.name),
                    format!("canonical metadata account {metadata_pda}"),
                ),
                Some(_) => report.warn(
                    "idl-drift",
                    format!("{}: on-chain IDL DIFFERS from local", artifact.name),
                    "clients consuming the on-chain IDL see a different interface than this build; the account already exists so an `idl init` would fail opaquely",
                    Some(format!(
                        "anchor idl upgrade {program_id} --filepath {}",
                        idl_path.display()
                    )),
                ),
                None => report.info(
                    "idl-drift",
                    format!("{}: on-chain IDL exists, no local IDL to compare", artifact.name),
                    format!("run `anchor build` to regenerate {}", idl_path.display()),
                ),
            },
            OnChainIdl::NotJson => report.warn(
                "idl-drift",
                format!("{}: on-chain IDL is CORRUPT (not valid JSON)", artifact.name),
                "typical cause: an interrupted chunked write left a partial IDL; `idl upgrade` against it keeps failing",
                Some(format!(
                    "anchor idl close {program_id}\nanchor idl init {program_id} --filepath {}",
                    idl_path.display()
                )),
            ),
            OnChainIdl::NotComparable(why) => report.info(
                "idl-drift",
                format!("{}: on-chain IDL not compared", artifact.name),
                why,
            ),
        }
    }
}

enum OnChainIdl {
    Json(serde_json::Value),
    NotJson,
    NotComparable(String),
}

/// Parse a Program Metadata account: 96-byte header (discriminator, program,
/// authority, flags, seed, encoding, compression, format, data_source,
/// data_length LE) followed by the document bytes.
fn on_chain_idl_json(data: &[u8]) -> OnChainIdl {
    use std::io::Read;
    if data.len() < facts::METADATA_HEADER_LEN || data[0] != 2 {
        return OnChainIdl::NotComparable(format!(
            "account is not an initialized metadata account (discriminator {:?})",
            data.first()
        ));
    }
    let data_source = data[86];
    if data_source != 0 {
        return OnChainIdl::NotComparable(
            "IDL is stored as a url/external pointer, not directly on chain".into(),
        );
    }
    let declared_len = u32::from_le_bytes([data[87], data[88], data[89], data[90]]) as usize;
    let body = &data[facts::METADATA_HEADER_LEN..];
    let body = &body[..declared_len.min(body.len())];
    let inflated: Vec<u8> = match data[84] {
        0 => body.to_vec(),
        1 => {
            let mut out = Vec::new();
            match flate2::read::GzDecoder::new(body).read_to_end(&mut out) {
                Ok(_) => out,
                Err(_) => return OnChainIdl::NotJson,
            }
        }
        2 => {
            let mut out = Vec::new();
            match flate2::read::ZlibDecoder::new(body).read_to_end(&mut out) {
                Ok(_) => out,
                Err(_) => return OnChainIdl::NotJson,
            }
        }
        other => {
            return OnChainIdl::NotComparable(format!("unknown compression tag {other}"));
        }
    };
    match serde_json::from_slice(&inflated) {
        Ok(value) => OnChainIdl::Json(value),
        Err(_) => OnChainIdl::NotJson,
    }
}

fn wallet_pubkey(project: &Project) -> Option<String> {
    // `~` in Anchor.toml wallet paths is common and the OS never expands it —
    // audit pass 1 found the old `root.join(raw)` silently broke on it,
    // skipping the balance/authority checks without a trace.
    let wallet_path = project.wallet_path()?;
    keypair_pubkey(&wallet_path).ok()
}

/// Programdata is zero-padded past the program's end by extends, and a .so can
/// itself end in zeros — equality must ignore both tails.
fn strip_trailing_zeros(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    &bytes[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_trailing_zeros_ignores_padding_but_keeps_interior_zeros() {
        assert_eq!(strip_trailing_zeros(&[1, 0, 2, 0, 0]), &[1, 0, 2]);
        assert_eq!(strip_trailing_zeros(&[0, 0, 0]), &[] as &[u8]);
        assert_eq!(strip_trailing_zeros(&[]), &[] as &[u8]);
        assert_eq!(strip_trailing_zeros(&[7]), &[7]);
    }

    #[test]
    fn padded_deploy_equals_local_build_after_stripping() {
        let local = [0x7f, b'E', b'L', b'F', 9, 9, 0];
        let mut deployed = local.to_vec();
        deployed.extend_from_slice(&[0; 10240]); // post-extend padding
        assert_eq!(
            strip_trailing_zeros(&deployed),
            strip_trailing_zeros(&local)
        );
    }
}
