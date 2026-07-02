# Canary matrix results

Machine: macOS · anchor CLI 1.1.2 · solana-cli 4.1.0 (Agave) · platform-tools v1.54 · run 2026-07-03.

Matrix intent per canary:

| id | variation | expectation |
|---|---|---|
| c01 | vanilla `anchor init` | baseline: everything green; devnet deploy+close |
| c02 | + litesvm 0.12 (dev) | resolves; sondir warns arch-litesvm on default (v0) .so |
| c03 | + litesvm 0.13.1 (dev) | resolves (no MagicBlock present) |
| c04 | + ephemeral-rollups-sdk 0.15.5 (anchor,vrf) | resolves; instruction 3.4 wave |
| c05 | + MagicBlock vrf + litesvm 0.13.1 | RESOLVE FAIL (=3.2.0 vs ^3.4) — sondir dep-conflict |
| c06 | + litesvm 0.13.1 + instructions-sysvar 3.0.1 | RESOLVE FAIL (^3.4 vs =3.2.0) |
| c07 | + litesvm 0.13.1 + instructions-sysvar =3.0.0 | resolves (3.0.0 rides ^3.0) |
| c08 | + anchor-spl (token, token_2022) | resolves |
| c09 | + spl-token-interface 3.0.0 | resolves |
| c10 | + blake3 latest | probe: does platform-tools v1.54 cargo handle edition2024? (historic breaker) |
| c11 | + light-sdk latest | probe Light Protocol dep tree vs anchor 1.1.2 |
| c12 | + pyth-solana-receiver-sdk latest | probe oracle SDK pins |
| c13 | + switchboard-on-demand latest | probe oracle SDK pins |
| c14 | + mpl-core latest | probe Metaplex vs anchor 1.1.2 |
| c15 | build --arch v1, deploy to devnet | EXPECT cluster rejection (v1 never deploy-enabled) — empirical proof |
| c16 | build --arch v3, deploy to devnet | deploys (SBPFv3 gate active) |
| c17 | deploy, grow binary <10240, upgrade | EXPECT SIMD-0431 failure; sondir must predict it pre-flight |
| c18 | two-program workspace | both build; deploy both |
| c19 | + solana-program 1.18 (legacy major) | probe major-mixing behavior |
| c20 | anchor-lang 0.31.1 under CLI 1.1.2 | mismatch warning; build probe |

## Runs

| id | resolve | native check | sondir (offline) | notes |
|---|---|---|---|---|
| c01 | OK | OK | 0 fail / 0 warn | — |
| c02 | OK | OK | 0 fail / 0 warn | — |
| c03 | OK | OK | 0 fail / 0 warn | — |
| c04 | OK | OK | 0 fail / 0 warn | — |
| c05 | FAIL | - | 2 fail / 0 warn (after sondir fix) | error: failed to select a version for `solana-instruction`.     ... required by package `solana-system-interface v3.2.0` all possible versions conflict with previously selected packages.  |

## Discoveries

1. **`anchor init` (CLI 1.1.2) templates ship `litesvm = "0.13.1"` in dev-dependencies** — so any fresh project + MagicBlock vrf is unresolvable out of the box (c05). Template-level breakage; strengthens the upstream litesvm issue.
2. **sondir bug found+fixed by c05**: a failed `cargo add` leaves a STALE lockfile (here: litesvm 0.10.0 in lock vs 0.13.1 declared) — dep checks must read declared manifests too, and a generic `resolve` probe (cargo metadata) is mandatory. Shipped as checks `resolve` + declared-deps fallback.
| c06 | FAIL | - | 1 fail / 0 warn | error: failed to select a version for `solana-instruction`.     ... required by package `litesvm v0.13.1` all possible versions conflict with previously selected packages.  |
| c07 | OK | OK | 0 fail / 0 warn | — |
| c08 | OK | OK | 0 fail / 0 warn | — |
| c09 | OK | OK | 0 fail / 0 warn | — |
| c10 | OK | OK | 0 fail / 0 warn | — |
| c11 | OK | OK | 0 fail / 0 warn | — |
| c12 | OK | OK | 0 fail / 0 warn | — |
| c13 | OK | OK | 0 fail / 0 warn | — |
| c14 | OK | OK | 0 fail / 0 warn | — |
| c19 | FAIL | - | 1 fail / 0 warn | error: failed to select a version for `zeroize`.     ... required by package `curve25519-dalek v3.2.1` all possible versions conflict with previously selected packages.  |
| c20 | OK | FAIL | 0 fail / 1 warn | error[E0308]: mismatched types error: could not compile `c20` (lib) due to 1 previous error; 2 warnings emitted  |
| c15 | OK | OK | 0 fail / 0 warn | — |
| c16 | OK | OK | 0 fail / 0 warn | — |
| c17 | OK | OK | 0 fail / 0 warn | — |
| c18 | OK | OK | 0 fail / 0 warn | — |
