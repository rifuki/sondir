//! Curated facts that no Cargo metadata or RPC schema expresses.
//!
//! The DATA lives in `facts/facts.toml` (embedded at build time, overridable at
//! runtime with `--facts <path>`), so entries can be corrected or extended
//! without a recompile — and eventually PR'd in a community facts repo. Logic
//! that interprets the data stays here.

use std::path::Path;
use std::sync::OnceLock;

use anyhow::{Context, Result};
use serde::Deserialize;

const EMBEDDED: &str = include_str!("../facts/facts.toml");

static FACTS: OnceLock<FactsFile> = OnceLock::new();

#[derive(Debug, Deserialize)]
pub struct FactsFile {
    #[serde(default)]
    pub gates: Vec<FeatureGate>,
    #[serde(default)]
    pub conflicts: Vec<KnownConflict>,
    #[serde(default)]
    pub litesvm_runtimes: Vec<LitesvmRuntime>,
}

#[derive(Debug, Deserialize)]
pub struct FeatureGate {
    pub address: String,
    pub simd: String,
    pub name: String,
    pub consequence: String,
}

#[derive(Debug, Deserialize)]
pub struct KnownConflict {
    pub id: String,
    pub a: String,
    pub b: String,
    pub why: String,
    pub fix: String,
}

#[derive(Debug, Deserialize)]
pub struct LitesvmRuntime {
    pub prefix: String,
    pub arch_ok: Vec<u32>,
    pub note: String,
}

/// Load the facts database once: an explicit `--facts` path, or the embedded
/// copy. Accessors fall back to the embedded copy if load was never called
/// (tests, library use).
pub fn load(override_path: Option<&Path>) -> Result<()> {
    let parsed = match override_path {
        Some(path) => {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("cannot read facts file {}", path.display()))?;
            toml::from_str(&raw).context("facts file did not parse")?
        }
        None => embedded(),
    };
    let _ = FACTS.set(parsed);
    Ok(())
}

fn embedded() -> FactsFile {
    toml::from_str(EMBEDDED).expect("embedded facts.toml must parse (validated by tests)")
}

fn facts() -> &'static FactsFile {
    FACTS.get_or_init(embedded)
}

pub fn gates() -> &'static [FeatureGate] {
    &facts().gates
}

pub fn conflicts() -> &'static [KnownConflict] {
    &facts().conflicts
}

pub fn conflict(id: &str) -> Option<&'static KnownConflict> {
    facts().conflicts.iter().find(|c| c.id == id)
}

pub fn litesvm_runtime(version: &str) -> Option<&'static LitesvmRuntime> {
    facts()
        .litesvm_runtimes
        .iter()
        .find(|runtime| version.starts_with(&runtime.prefix))
}

/// BPF upgradeable loader (loader-v3) program id.
pub const UPGRADEABLE_LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";

/// Bytes of bincode metadata before the ELF in a ProgramData account
/// (`UpgradeableLoaderState::size_of_programdata_metadata()`).
pub const PROGRAMDATA_METADATA_LEN: u64 = 45;

/// Bytes of bincode metadata before the ELF in a Buffer account.
pub const BUFFER_METADATA_LEN: u64 = 37;

/// SIMD-0431: Loader-v3 minimum extend size, in bytes.
pub const MIN_EXTEND_BYTES: u64 = 10_240;

/// Program Metadata program (stores canonical IDLs for anchor 1.x).
pub const PROGRAM_METADATA_PROGRAM: &str = "ProgM6JCCvbYkfKqJYHePx4xxSUSqJp7rh8Lyv7nk7S";

/// Canonical IDL metadata PDA seed: "idl" zero-padded to the program's
/// SEED_LEN of 16 (EMPIRICAL, devnet 2026-07-04: raw "idl" derives an
/// account that does not exist; the padded form matches anchor's writes).
pub const IDL_SEED_PADDED: [u8; 16] = *b"idl\0\0\0\0\0\0\0\0\0\0\0\0\0";

/// Bytes of Program Metadata `Header` before the data (repr(C), align 1).
pub const METADATA_HEADER_LEN: usize = 96;

/// SBPF arch flag (ELF e_flags word at byte offset 48) vs cluster deploy rules.
///
/// EMPIRICAL (canary c15, devnet 2026-07-03): with the SBPFv3 gate active the
/// e_flags direct mapping accepts v1/v2 deploys too — the old "v1/v2 were never
/// enabled" belief is obsolete.
pub fn arch_deployable(
    flag: u32,
    sbpf_v3_active: bool,
    simd_0500_active: bool,
) -> std::result::Result<(), String> {
    match flag {
        0 => {
            if simd_0500_active {
                Err("SBPF v0 deploys are disabled on this cluster (SIMD-0500 active) — build with --arch v3".into())
            } else {
                Ok(())
            }
        }
        1 | 2 => {
            if simd_0500_active {
                Err(format!(
                    "SBPF v{flag} deploys are disabled on this cluster (SIMD-0500 active) — build with --arch v3"
                ))
            } else if sbpf_v3_active {
                Ok(())
            } else {
                Err(format!(
                    "SBPF v{flag} is not deployable on clusters without the SBPFv3 e_flags-mapping gate"
                ))
            }
        }
        3 => {
            if sbpf_v3_active {
                Ok(())
            } else {
                Err("SBPF v3 gate is not active on this cluster — v3 .so will be rejected".into())
            }
        }
        other => Err(format!("unknown SBPF arch flag {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_facts_parse_and_are_complete() {
        let facts = embedded();
        assert_eq!(facts.gates.len(), 3);
        assert_eq!(facts.conflicts.len(), 3);
        assert_eq!(facts.litesvm_runtimes.len(), 2);
        assert!(conflict("litesvm-magicblock").is_some());
    }

    #[test]
    fn v0_is_deployable_until_simd_0500_activates() {
        assert!(arch_deployable(0, true, false).is_ok());
    }

    #[test]
    fn v0_dies_when_simd_0500_activates() {
        assert!(arch_deployable(0, true, true).is_err());
    }

    #[test]
    fn v1_v2_deployable_only_under_the_v3_gate() {
        assert!(arch_deployable(1, true, false).is_ok());
        assert!(arch_deployable(2, true, false).is_ok());
        assert!(arch_deployable(1, false, false).is_err());
        assert!(arch_deployable(1, true, true).is_err());
    }

    #[test]
    fn v3_requires_its_gate() {
        assert!(arch_deployable(3, false, false).is_err());
        assert!(arch_deployable(3, true, false).is_ok());
    }

    #[test]
    fn litesvm_012_executes_v1_v2_only() {
        let runtime = litesvm_runtime("0.12.0").expect("known version");
        assert_eq!(runtime.arch_ok, vec![1, 2]);
    }

    #[test]
    fn litesvm_013_executes_v3() {
        let runtime = litesvm_runtime("0.13.1").expect("known version");
        assert_eq!(runtime.arch_ok, vec![3]);
    }

    #[test]
    fn unknown_litesvm_yields_no_claim() {
        assert!(litesvm_runtime("0.14.0").is_none());
    }
}
