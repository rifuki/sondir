//! Curated facts that no Cargo metadata or RPC schema expresses.
//!
//! This is the seed of the facts database. Each entry carries the consequence
//! it implies so checks stay declarative. Verified 2026-07-02 against devnet /
//! testnet / mainnet (`solana feature status`) and the raflux toolchain lab.

/// BPF upgradeable loader (loader-v3) program id.
pub const UPGRADEABLE_LOADER: &str = "BPFLoaderUpgradeab1e11111111111111111111111";

/// Bytes of bincode metadata before the ELF in a ProgramData account
/// (`UpgradeableLoaderState::size_of_programdata_metadata()`).
pub const PROGRAMDATA_METADATA_LEN: u64 = 45;

/// Bytes of bincode metadata before the ELF in a Buffer account.
pub const BUFFER_METADATA_LEN: u64 = 37;

/// SIMD-0431: Loader-v3 minimum extend size, in bytes.
pub const MIN_EXTEND_BYTES: u64 = 10_240;

pub struct FeatureGate {
    pub address: &'static str,
    pub simd: &'static str,
    pub name: &'static str,
    /// What an ACTIVE gate means for a deployer.
    pub consequence: &'static str,
}

pub const GATES: &[FeatureGate] = &[
    FeatureGate {
        address: "5cC3foj77CWun58pC51ebHFUWavHWKarWyR5UUik7dnC",
        simd: "SIMD-0178/0189/0377",
        name: "SBPFv3 deployment and execution",
        consequence: "arch v3 .so files are deployable/executable on this cluster",
    },
    FeatureGate {
        address: "YbbRLkvenrocjGPGyoQE4wjnvYzTgfsk38NFmcYK7a5",
        simd: "SIMD-0431",
        name: "Loader-v3 minimum extend program size",
        consequence: "ExtendProgram must add >= 10240 bytes (or extend to max); small auto-extends on upgrade FAIL",
    },
    FeatureGate {
        address: "B8JJXCy5amZyWG9r7EnUYLwzXSXTxG7GZ1qZ1qggo83g",
        simd: "SIMD-0500",
        name: "Disable deployment of SBPF v0, v1 and v2 programs",
        consequence: "only arch v3 .so files remain deployable",
    },
];

/// SBPF arch flag (ELF e_flags word at byte offset 48) semantics per cluster.
///
/// v1/v2 were never enabled for cluster deployment: they exist only for local
/// runtimes (litesvm/mollusk).
pub fn arch_deployable(
    flag: u32,
    sbpf_v3_active: bool,
    simd_0500_active: bool,
) -> Result<(), String> {
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
                // EMPIRICAL (canary c15, devnet 2026-07-03): with the SBPFv3
                // gate active the e_flags direct mapping accepts v1/v2 deploys
                // too — the old "v1/v2 were never enabled" belief is obsolete.
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

/// What SBPF arch a given litesvm crate version can execute.
///
/// 0.12.x embeds the Agave 3.1.14 runtime: executes v1/v2; chokes on v0 emitted
/// by platform-tools >= v1.54 ("Access violation ... 0x8") and on v3.
/// 0.13.x embeds Agave 4.0: executes v3 (but see the MagicBlock conflict fact).
pub struct LitesvmRuntime {
    pub arch_ok: &'static [u32],
    pub note: &'static str,
}

pub fn litesvm_runtime(version: &str) -> Option<LitesvmRuntime> {
    if version.starts_with("0.12.") {
        Some(LitesvmRuntime {
            arch_ok: &[1, 2],
            note: "litesvm 0.12 = Agave 3.1.14 runtime: test .so must be --arch v1 (or v2); v0 from platform-tools >=1.54 and v3 both fail",
        })
    } else if version.starts_with("0.13.") {
        Some(LitesvmRuntime {
            arch_ok: &[3],
            note: "litesvm 0.13 = Agave 4.0 runtime (SBPF v3). NOTE: its solana-instruction =3.2.0 exact pin conflicts with MagicBlock vrf-sdk (needs ^3.4) — unresolvable in one graph as of 2026-07-02",
        })
    } else {
        None
    }
}

/// Known unresolvable dependency pairs (same-major requirement conflicts).
pub struct KnownConflict {
    pub a: &'static str,
    pub b: &'static str,
    pub why: &'static str,
}

pub const KNOWN_CONFLICTS: &[KnownConflict] = &[
    KnownConflict {
        a: "litesvm >=0.13",
        b: "ephemeral-rollups-sdk (vrf) / ephemeral-vrf-sdk >=0.3",
        why: "litesvm 0.13.x pins solana-instruction =3.2.0 (Agave 4.0 wave) while the MagicBlock chain requires ^3.4 (Agave 4.1 wave); cargo cannot unify same-major exact vs caret. Unlocks when litesvm ships an Agave-4.1-wave release. NOTE: `anchor init` (CLI 1.1.2) templates ship litesvm 0.13.1, so fresh-project + MagicBlock hits this immediately (canary c05).",
    },
    KnownConflict {
        a: "litesvm >=0.13",
        b: "solana-instructions-sysvar >=3.0.1",
        why: "solana-instructions-sysvar 3.0.1 requires solana-instruction ^3.4.0; litesvm 0.13.x pins =3.2.0 (canary c06). Pin solana-instructions-sysvar to =3.0.0 (rides ^3.0.0) or drop litesvm to 0.12.x.",
    },
    KnownConflict {
        a: "solana-program 1.x (legacy)",
        b: "modern anchor workspace (solana-* 3.x wave)",
        why: "the legacy 1.x line drags curve25519-dalek 3.2.1 whose zeroize pin clashes with the modern tree (canary c19: `failed to select a version for zeroize`). Remove the direct solana-program dep and use anchor_lang re-exports (or granular solana-* 3.x crates).",
    },
];

#[cfg(test)]
mod tests {
    use super::*;

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
        // empirically corrected by canary c15 (devnet accepted arch v1)
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
        assert_eq!(runtime.arch_ok, &[1, 2]);
    }

    #[test]
    fn litesvm_013_executes_v3() {
        let runtime = litesvm_runtime("0.13.1").expect("known version");
        assert_eq!(runtime.arch_ok, &[3]);
    }

    #[test]
    fn unknown_litesvm_yields_no_claim() {
        assert!(litesvm_runtime("0.14.0").is_none());
    }
}
