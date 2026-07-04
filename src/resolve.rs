//! `sondir resolve` — find a mutually-compatible version set for a selection of
//! Solana ecosystem dependencies.
//!
//! Mechanism: synthesize a throwaway manifest with the selection at `*`, let
//! CARGO'S OWN resolver do the search (`cargo generate-lockfile`), and read the
//! answer out of the lockfile. On failure, apply known remedies from the facts
//! DB (e.g. "pin litesvm <0.13") and retry — so the answer becomes "works if
//! you pin X", not just "conflict".

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::facts;
use crate::project::parse_lockfile;

/// Friendly names for the deps people actually reach for.
pub struct Alias {
    pub names: &'static [&'static str],
    pub krate: &'static str,
    pub features: &'static [&'static str],
    pub note: &'static str,
}

pub const ALIASES: &[Alias] = &[
    Alias {
        names: &["anchor", "anchor-lang"],
        krate: "anchor-lang",
        features: &[],
        note: "Anchor framework (program side)",
    },
    Alias {
        names: &["anchor-spl", "spl"],
        krate: "anchor-spl",
        features: &[],
        note: "Anchor SPL helpers",
    },
    Alias {
        names: &["litesvm"],
        krate: "litesvm",
        features: &[],
        note: "in-process SVM test runtime",
    },
    Alias {
        names: &["magicblock", "vrf", "ephemeral-rollups-sdk"],
        krate: "ephemeral-rollups-sdk",
        features: &["anchor", "vrf"],
        note: "MagicBlock ephemeral rollups + VRF",
    },
    Alias {
        names: &["light", "light-protocol", "light-sdk"],
        krate: "light-sdk",
        features: &[],
        note: "Light Protocol (ZK compression)",
    },
    Alias {
        names: &["pyth"],
        krate: "pyth-solana-receiver-sdk",
        features: &[],
        note: "Pyth price oracle receiver",
    },
    Alias {
        names: &["switchboard"],
        krate: "switchboard-on-demand",
        features: &[],
        note: "Switchboard on-demand oracle",
    },
    Alias {
        names: &["metaplex", "mpl-core"],
        krate: "mpl-core",
        features: &[],
        note: "Metaplex Core NFTs",
    },
    Alias {
        names: &["spl-token-interface"],
        krate: "spl-token-interface",
        features: &[],
        note: "SPL token state/interface types",
    },
    Alias {
        names: &["instructions-sysvar"],
        krate: "solana-instructions-sysvar",
        features: &[],
        note: "instructions sysvar introspection",
    },
    Alias {
        names: &["pinocchio"],
        krate: "pinocchio",
        features: &[],
        note: "zero-dependency program framework",
    },
    Alias {
        names: &["mollusk"],
        krate: "mollusk-svm",
        features: &[],
        note: "Mollusk SVM test harness",
    },
];

/// Crates whose resolved version reveals which "Agave wave" the set landed on.
const PIVOTS: &[&str] = &[
    "anchor-lang",
    "solana-instruction",
    "solana-pubkey",
    "solana-system-interface",
    "solana-account",
    "solana-program-runtime",
];

struct Selection {
    krate: String,
    req: String,
    features: Vec<String>,
}

/// Outcome of probing one synthetic manifest.
pub enum ProbeResult {
    Resolves(BTreeMap<String, String>),
    Conflicts(String),
}

/// Does this exact set of `(name, req, features)` deps resolve together?
/// Shared by `facts verify` (which EXPECTS conflicts to fail) and future
/// callers; `tag` keeps concurrent temp workspaces apart.
pub fn probe(deps: &[(String, String, Vec<String>)], tag: &str) -> Result<ProbeResult> {
    let selection: Vec<Selection> = deps
        .iter()
        .map(|(name, req, features)| Selection {
            krate: name.clone(),
            req: req.clone(),
            features: features.clone(),
        })
        .collect();
    let workdir = std::env::temp_dir().join(format!("sondir-probe-{tag}-{}", std::process::id()));
    write_workspace(&workdir, &selection)?;
    let output = Command::new("cargo")
        .args(["generate-lockfile", "--quiet"])
        .current_dir(&workdir)
        .output()
        .context("cargo not found on PATH")?;
    let result = if output.status.success() {
        let raw = fs::read_to_string(workdir.join("Cargo.lock"))?;
        ProbeResult::Resolves(parse_lockfile(&raw)?)
    } else {
        ProbeResult::Conflicts(String::from_utf8_lossy(&output.stderr).into_owned())
    };
    let _ = fs::remove_dir_all(&workdir);
    Ok(result)
}

#[derive(Serialize)]
pub struct Resolution {
    pub requested: Vec<String>,
    pub applied_pins: Vec<AppliedPin>,
    pub resolved: BTreeMap<String, String>,
    pub pivots: BTreeMap<String, String>,
    pub notes: Vec<String>,
    pub attempts: usize,
}

#[derive(Serialize, Clone)]
pub struct AppliedPin {
    pub krate: String,
    pub req: String,
    pub why: String,
}

/// What `resolve()` concluded — a compatible set, or an explained conflict.
/// Untagged so the JSON shapes stay identical to the pre-split `--json` output.
#[derive(Serialize)]
#[serde(untagged)]
pub enum Outcome {
    Resolved(Resolution),
    Failed(Failure),
}

#[derive(Serialize)]
pub struct Failure {
    pub error: String,
    pub requested: Vec<String>,
    pub applied_pins: Vec<AppliedPin>,
    pub cargo: Vec<String>,
    pub known_conflicts: Vec<String>,
}

/// A facts-driven retry: when resolution fails and the trigger matches, pin
/// `krate` to `req` and try again.
struct Remedy {
    requested_contains: &'static str,
    stderr_contains: &'static str,
    pin_crate: &'static str,
    pin_req: &'static str,
    why: &'static str,
}

const REMEDIES: &[Remedy] = &[
    Remedy {
        requested_contains: "litesvm",
        stderr_contains: "solana-instruction",
        pin_crate: "litesvm",
        pin_req: "<0.13",
        why: "litesvm 0.13.x pins solana-instruction =3.2.0 (Agave 4.0 wave) which conflicts with the instruction ^3.4 ecosystem (MagicBlock, instructions-sysvar 3.0.1+); 0.12.x has loose reqs — canary c05/c06",
    },
    Remedy {
        requested_contains: "solana-instructions-sysvar",
        stderr_contains: "solana-instruction",
        pin_crate: "solana-instructions-sysvar",
        pin_req: "=3.0.0",
        why: "instructions-sysvar 3.0.1+ requires solana-instruction ^3.4; =3.0.0 rides ^3.0 and coexists with litesvm 0.13's =3.2.0 pin — canary c06/c07",
    },
    Remedy {
        requested_contains: "mollusk-svm",
        stderr_contains: "solana-instruction",
        pin_crate: "mollusk-svm",
        pin_req: "<0.13",
        why: "mollusk-svm 0.13.x pins the Agave-4.0 interface wave via agave-syscalls 4.0.0, conflicting with the ^3.4 ecosystem; 0.12.x resolves — sweep discovery 2026-07-04",
    },
    Remedy {
        requested_contains: "mollusk-svm",
        stderr_contains: "solana-pubkey",
        pin_crate: "mollusk-svm",
        pin_req: "<0.13",
        why: "mollusk-svm 0.13.x pins the Agave-4.0 interface wave via agave-syscalls 4.0.0, conflicting with the ^3.4/4.x ecosystem; 0.12.x resolves — sweep discovery 2026-07-04",
    },
];

pub fn list_aliases() {
    println!("known selections (raw crate names also accepted):\n");
    for alias in ALIASES {
        let features = if alias.features.is_empty() {
            String::new()
        } else {
            format!(" (features: {})", alias.features.join(", "))
        };
        println!(
            "  {:24} -> {}{features}\n  {:24}    {}",
            alias.names.join(" | "),
            alias.krate,
            "",
            alias.note
        );
    }
}

pub fn run(names: &[String], json: bool) -> Result<i32> {
    match resolve(names)? {
        Outcome::Resolved(resolution) => {
            print_resolution(&resolution, json)?;
            Ok(0)
        }
        Outcome::Failed(failure) => {
            print_failure(&failure, json)?;
            Ok(1)
        }
    }
}

/// Resolution without printing (shared by the CLI and the MCP server).
pub fn resolve(names: &[String]) -> Result<Outcome> {
    let mut selection = build_selection(names);
    let requested: Vec<String> = selection.iter().map(|s| s.krate.clone()).collect();
    let mut applied: Vec<AppliedPin> = Vec::new();

    let workdir = std::env::temp_dir().join(format!("sondir-resolve-{}", std::process::id()));

    for attempt in 1..=1 + REMEDIES.len() {
        write_workspace(&workdir, &selection)?;
        let output = Command::new("cargo")
            .args(["generate-lockfile", "--quiet"])
            .current_dir(&workdir)
            .output()
            .context("cargo not found on PATH")?;

        if output.status.success() {
            let raw = fs::read_to_string(workdir.join("Cargo.lock"))?;
            let locked = parse_lockfile(&raw)?;
            let resolution = summarize(&requested, &applied, &locked, attempt);
            let _ = fs::remove_dir_all(&workdir);
            return Ok(Outcome::Resolved(resolution));
        }

        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        if let Some(remedy) = next_remedy(&requested, &stderr, &applied) {
            let pin = AppliedPin {
                krate: remedy.pin_crate.to_owned(),
                req: remedy.pin_req.to_owned(),
                why: remedy.why.to_owned(),
            };
            apply_pin(&mut selection, &pin);
            applied.push(pin);
            continue;
        }

        // Out of remedies: report the conflict with whatever facts explain it.
        let _ = fs::remove_dir_all(&workdir);
        return Ok(Outcome::Failed(build_failure(&requested, applied, &stderr)));
    }

    let _ = fs::remove_dir_all(&workdir);
    Ok(Outcome::Failed(build_failure(
        &requested,
        applied,
        "remedy loop exhausted",
    )))
}

fn build_selection(names: &[String]) -> Vec<Selection> {
    let mut selection: Vec<Selection> = Vec::new();
    for name in names {
        let lowered = name.to_lowercase();
        let (krate, features) = ALIASES
            .iter()
            .find(|alias| alias.names.contains(&lowered.as_str()))
            .map(|alias| (alias.krate.to_owned(), alias.features.to_vec()))
            .unwrap_or((lowered.clone(), Vec::new()));
        if let Some(existing) = selection.iter_mut().find(|s| s.krate == krate) {
            for feature in &features {
                if !existing.features.iter().any(|f| f == feature) {
                    existing.features.push((*feature).to_owned());
                }
            }
        } else {
            selection.push(Selection {
                krate,
                req: "*".into(),
                features: features.into_iter().map(str::to_owned).collect(),
            });
        }
    }
    selection
}

fn write_workspace(dir: &PathBuf, selection: &[Selection]) -> Result<()> {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir.join("src"))?;
    fs::write(dir.join("src/lib.rs"), "")?;
    let mut manifest = String::from(
        "[package]\nname = \"sondir-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    );
    for dep in selection {
        if dep.features.is_empty() {
            manifest.push_str(&format!("{} = \"{}\"\n", dep.krate, dep.req));
        } else {
            let features = dep
                .features
                .iter()
                .map(|f| format!("\"{f}\""))
                .collect::<Vec<_>>()
                .join(", ");
            manifest.push_str(&format!(
                "{} = {{ version = \"{}\", features = [{features}] }}\n",
                dep.krate, dep.req
            ));
        }
    }
    fs::write(dir.join("Cargo.toml"), manifest)?;
    Ok(())
}

fn next_remedy<'a>(
    requested: &[String],
    stderr: &str,
    applied: &[AppliedPin],
) -> Option<&'a Remedy> {
    REMEDIES.iter().find(|remedy| {
        requested.iter().any(|r| r == remedy.requested_contains)
            && stderr.contains(remedy.stderr_contains)
            && !applied.iter().any(|pin| pin.krate == remedy.pin_crate)
    })
}

fn apply_pin(selection: &mut [Selection], pin: &AppliedPin) {
    if let Some(dep) = selection.iter_mut().find(|s| s.krate == pin.krate) {
        dep.req = pin.req.clone();
    }
}

fn summarize(
    requested: &[String],
    applied: &[AppliedPin],
    locked: &BTreeMap<String, String>,
    attempts: usize,
) -> Resolution {
    let resolved = requested
        .iter()
        .filter_map(|krate| locked.get(krate).map(|v| (krate.clone(), v.clone())))
        .collect::<BTreeMap<_, _>>();
    let pivots = PIVOTS
        .iter()
        .filter_map(|krate| locked.get(*krate).map(|v| ((*krate).to_owned(), v.clone())))
        .collect::<BTreeMap<_, _>>();

    let mut notes = Vec::new();
    if let Some(version) = resolved.get("litesvm") {
        if let Some(runtime) = facts::litesvm_runtime(version) {
            notes.push(format!("litesvm {version}: {}", runtime.note));
        }
    }
    if let Some(instruction) = pivots.get("solana-instruction") {
        notes.push(format!(
            "wave marker: solana-instruction {instruction} ({})",
            if instruction.starts_with("3.4") || instruction.starts_with("3.5") {
                "Agave 4.1 interface wave"
            } else if instruction.starts_with("3.2") || instruction.starts_with("3.3") {
                "Agave 4.0 interface wave"
            } else {
                "unrecognized wave"
            }
        ));
    }

    Resolution {
        requested: requested.to_vec(),
        applied_pins: applied.to_vec(),
        resolved,
        pivots,
        notes,
        attempts,
    }
}

fn print_resolution(resolution: &Resolution, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(resolution)?);
        return Ok(());
    }
    println!(
        "✓ compatible set found (attempt {} of up to {})\n",
        resolution.attempts,
        1 + REMEDIES.len()
    );
    if !resolution.applied_pins.is_empty() {
        println!("required pins (put these in Cargo.toml):");
        for pin in &resolution.applied_pins {
            println!("  {} = \"{}\"", pin.krate, pin.req);
            println!("    why: {}", pin.why);
        }
        println!();
    }
    println!("requested:");
    for (krate, version) in &resolution.resolved {
        println!("  {krate} = \"{version}\"");
    }
    if !resolution.pivots.is_empty() {
        println!("\npivot crates (which wave you landed on):");
        for (krate, version) in &resolution.pivots {
            println!("  {krate} {version}");
        }
    }
    for note in &resolution.notes {
        println!("\nnote: {note}");
    }
    Ok(())
}

fn build_failure(requested: &[String], applied: Vec<AppliedPin>, stderr: &str) -> Failure {
    let cargo: Vec<String> = stderr
        .lines()
        .filter(|line| {
            line.contains("failed to select")
                || line.contains("required by")
                || line.contains("previously selected")
                || line.contains("which satisfies")
        })
        .take(6)
        .map(str::to_owned)
        .collect();
    let known_conflicts: Vec<String> = facts::conflicts()
        .iter()
        .filter(|conflict| {
            requested
                .iter()
                .any(|r| conflict.a.contains(r.as_str()) || conflict.b.contains(r.as_str()))
        })
        .map(|conflict| conflict.why.clone())
        .collect();
    Failure {
        error: "no compatible set found".into(),
        requested: requested.to_vec(),
        applied_pins: applied,
        cargo,
        known_conflicts,
    }
}

fn print_failure(failure: &Failure, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(failure)?);
        return Ok(());
    }
    println!(
        "✗ no compatible set found for: {}",
        failure.requested.join(", ")
    );
    if !failure.applied_pins.is_empty() {
        println!("\npins already tried:");
        for pin in &failure.applied_pins {
            println!("  {} = \"{}\"", pin.krate, pin.req);
        }
    }
    if !failure.cargo.is_empty() {
        println!("\ncargo says:");
        for line in &failure.cargo {
            println!("  {}", line.trim());
        }
    }
    for why in &failure.known_conflicts {
        println!("\nknown conflict: {why}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases_map_and_merge_features() {
        let selection = build_selection(&[
            "magicblock".into(),
            "vrf".into(),
            "anchor".into(),
            "some-raw-crate".into(),
        ]);
        assert_eq!(selection.len(), 3);
        let magicblock = selection
            .iter()
            .find(|s| s.krate == "ephemeral-rollups-sdk")
            .expect("alias mapped");
        assert_eq!(magicblock.features, vec!["anchor", "vrf"]);
        assert!(selection.iter().any(|s| s.krate == "some-raw-crate"));
    }

    #[test]
    fn remedies_do_not_repeat() {
        let requested = vec!["litesvm".to_owned()];
        let applied = vec![AppliedPin {
            krate: "litesvm".into(),
            req: "<0.13".into(),
            why: String::new(),
        }];
        assert!(next_remedy(&requested, "solana-instruction conflict", &applied).is_none());
    }

    #[test]
    fn manifest_renders_features() {
        let dir = std::env::temp_dir().join(format!("sondir-manifest-test-{}", std::process::id()));
        let selection = build_selection(&["magicblock".into()]);
        write_workspace(&dir, &selection).expect("write");
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).expect("read");
        assert!(manifest.contains(
            "ephemeral-rollups-sdk = { version = \"*\", features = [\"anchor\", \"vrf\"] }"
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
