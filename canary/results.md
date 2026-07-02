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
