//! `sondir watch` — has an upstream event fired that unlocks held-back upgrades?
//!
//! The canary campaign produced a trigger list ("when litesvm ships its
//! Agave-4.1 wave, bump five things at once"). This command checks those
//! triggers NOW: crates.io for release/requirement changes, the cluster for
//! gate activations. Run it from CI or a cron to get notified the day an
//! unlock lands.

use anyhow::{anyhow, Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::facts;
use crate::rpc::RpcClient;

#[derive(Serialize)]
pub struct Trigger {
    pub id: &'static str,
    pub fired: bool,
    pub status: String,
    pub then: &'static str,
}

pub fn run(rpc_url: &str, json: bool) -> Result<i32> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(15))
        .build();
    let mut triggers = Vec::new();

    // 1. litesvm ships an Agave-4.1-wave release (solana-instruction req leaves =3.2.0).
    triggers.push(litesvm_wave_trigger(&agent).unwrap_or_else(|err| Trigger {
        id: "litesvm-agave41-wave",
        fired: false,
        status: format!("check failed: {err:#}"),
        then: LITESVM_THEN,
    }));

    // 2. SIMD-0500 activates (v0-v2 deploys die) on the target cluster.
    triggers.push(simd_0500_trigger(rpc_url));

    // 3. anchor-lang crosses to the pubkey-4 / solana-address interface wave.
    triggers.push(anchor_wave_trigger(&agent).unwrap_or_else(|err| Trigger {
        id: "anchor-pubkey4-wave",
        fired: false,
        status: format!("check failed: {err:#}"),
        then: ANCHOR_THEN,
    }));

    let any_fired = triggers.iter().any(|t| t.fired);
    if json {
        println!("{}", serde_json::to_string_pretty(&triggers)?);
    } else {
        for trigger in &triggers {
            let glyph = if trigger.fired {
                "🔓 FIRED"
            } else {
                "…waiting"
            };
            println!("{glyph}  [{}]", trigger.id);
            println!("      now:  {}", trigger.status);
            println!("      then: {}\n", trigger.then);
        }
    }
    // Exit 3 when something fired so CI/cron can alert on it.
    Ok(if any_fired { 3 } else { 0 })
}

const LITESVM_THEN: &str = "bump litesvm past 0.12, bump solana-account/-rent/-message/-transaction to their 4.x majors, flip test builds from --arch v1 to v3 — the whole dual-arch split retires";

fn litesvm_wave_trigger(agent: &ureq::Agent) -> Result<Trigger> {
    let max = crates_io_max_version(agent, "litesvm")?;
    let instruction_req = crates_io_dep_req(agent, "litesvm", &max, "solana-instruction")?
        .unwrap_or_else(|| "<none>".into());
    // The 4.0-wave pin we're stuck behind is "=3.2.0"; anything allowing 3.4+
    // means the wave shipped.
    let fired = instruction_req != "=3.2.0" && !instruction_req.starts_with("=3.3");
    Ok(Trigger {
        id: "litesvm-agave41-wave",
        fired,
        status: format!("litesvm {max} requires solana-instruction {instruction_req}"),
        then: LITESVM_THEN,
    })
}

fn simd_0500_trigger(rpc_url: &str) -> Trigger {
    let then = "v0–v2 deploys die on this cluster: deploy artifacts MUST be --arch v3 (raflux already builds v3 via scripts/devnet)";
    let gate = facts::gates().iter().find(|g| g.simd == "SIMD-0500");
    let Some(gate) = gate else {
        return Trigger {
            id: "simd-0500-activation",
            fired: false,
            status: "gate missing from facts db".into(),
            then,
        };
    };
    let rpc = RpcClient::new(rpc_url);
    match rpc.feature_active(&gate.address) {
        Ok(Some(true)) => Trigger {
            id: "simd-0500-activation",
            fired: true,
            status: format!("ACTIVE on {rpc_url}"),
            then,
        },
        Ok(_) => Trigger {
            id: "simd-0500-activation",
            fired: false,
            status: format!("inactive on {rpc_url}"),
            then,
        },
        Err(err) => Trigger {
            id: "simd-0500-activation",
            fired: false,
            status: format!("check failed: {err:#}"),
            then,
        },
    }
}

const ANCHOR_THEN: &str = "program-side solana-* interface majors unlock (pubkey/address 4.x wave) — re-run the whole compatibility matrix";

fn anchor_wave_trigger(agent: &ureq::Agent) -> Result<Trigger> {
    let max = crates_io_max_version(agent, "anchor-lang")?;
    let pubkey_req = crates_io_dep_req(agent, "anchor-lang", &max, "solana-pubkey")?;
    let address_req = crates_io_dep_req(agent, "anchor-lang", &max, "solana-address")?;
    let fired = address_req.is_some()
        || pubkey_req
            .as_deref()
            .is_some_and(|req| req.trim_start_matches(['^', '=', '~']).starts_with('4'));
    let status = match (&pubkey_req, &address_req) {
        (_, Some(req)) => format!("anchor-lang {max} uses solana-address {req}"),
        (Some(req), None) => format!("anchor-lang {max} requires solana-pubkey {req}"),
        (None, None) => format!("anchor-lang {max}: no pubkey/address dep visible"),
    };
    Ok(Trigger {
        id: "anchor-pubkey4-wave",
        fired,
        status,
        then: ANCHOR_THEN,
    })
}

fn crates_io_max_version(agent: &ureq::Agent, krate: &str) -> Result<String> {
    let body: Value = agent
        .get(&format!("https://crates.io/api/v1/crates/{krate}"))
        .set("user-agent", "sondir (https://github.com/rifuki/sondir)")
        .call()
        .with_context(|| format!("crates.io lookup for {krate} failed"))?
        .into_json()?;
    body["crate"]["max_stable_version"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("crates.io: no max_stable_version for {krate}"))
}

fn crates_io_dep_req(
    agent: &ureq::Agent,
    krate: &str,
    version: &str,
    dep: &str,
) -> Result<Option<String>> {
    let body: Value = agent
        .get(&format!(
            "https://crates.io/api/v1/crates/{krate}/{version}/dependencies"
        ))
        .set("user-agent", "sondir (https://github.com/rifuki/sondir)")
        .call()
        .with_context(|| format!("crates.io deps lookup for {krate} {version} failed"))?
        .into_json()?;
    Ok(body["dependencies"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["crate_id"] == dep && entry["kind"] == "normal")
        .and_then(|entry| entry["req"].as_str())
        .map(str::to_owned))
}
