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

## Deploy runs (devnet via dedicated RPC, canary wallet D87G9f…, all programs closed after — net cost ≈ 0.012 SOL)

| id | flow | result |
|---|---|---|
| c01 | deploy v0 → close | ✓ deployed `5HWuxo…`, closed, rent reclaimed |
| c10 | `anchor build` (cargo-build-sbf, platform-tools v1.54) with blake3 latest | ✓ **edition2024 breakage is GONE in v1.54** (historic v1.48 blocker resolved) |
| c15 | build `--arch v1` → deploy | **DEPLOYED** `DV15iy…` — the "v1/v2 were never deploy-enabled" belief is WRONG under the SBPFv3 e_flags gate; sondir fact corrected + false-positive fixed |
| c16 | build `--arch v3` → deploy → close | ✓ `zjQgsF…` |
| c17 | deploy → grow +2088 B → sondir predict → upgrade → fix → upgrade | ✓ full SIMD-0431 trap cycle: sondir FAILED it pre-flight ("upgrade WILL fail"), anchor then failed exactly so (stranding 1.037 SOL, which sondir detected), `extend 10240` per sondir's fix, upgrade succeeded, closed |
| c18 | two-program workspace | ✓ both built, both deployed (`5QLdY1…`, `EmcGSw…`), both closed |

## Discoveries (batch 2 + deploys)

3. **c06**: litesvm 0.13.1 × `solana-instructions-sysvar 3.0.1` unresolvable; `=3.0.0` (c07) is the escape hatch. → facts + dep-conflict check.
4. **c11–c14**: Light Protocol (light-sdk), Pyth (pyth-solana-receiver-sdk), Switchboard (switchboard-on-demand), Metaplex (mpl-core) all resolve + type-check against anchor-lang 1.1.2. Good ecosystem health at the resolve level.
5. **c19**: legacy `solana-program 1.18` in a modern workspace fails resolution on `zeroize` via `curve25519-dalek 3.2.1` — the classic; now a named dep-conflict with fix.
6. **c20**: anchor-lang 0.31.1 under CLI 1.1.2 resolves but the v1 template source fails to compile (E0308) — the toolchain-anchor warn fires; template/API skew is the real hazard.
7. **c15 correction**: devnet ACCEPTED an arch-v1 deploy — with the SBPFv3 gate active, e_flags maps directly and v0–v3 are all deployable (until SIMD-0500 kills 0–2). `facts::arch_deployable` corrected; test updated.
8. **edition2024 (c10)**: platform-tools v1.54's bundled cargo handles `edition = "2024"` crates (blake3 latest builds for SBF). The Jan-2026 pinning ritual (blake3=1.8.2 etc.) is obsolete on v1.54+.

## Batch 3 — upstream thaw re-check (2026-07-27)

Trigger: `facts verify` reported 6 of 7 conflicts **stale**. Re-checked against crates.io
metadata and a live VM probe rather than trusting either the DB or the "it's fixed now" claim.

| id | probe | result |
|---|---|---|
| — | litesvm 0.14.0 (2026-07-13) dep metadata | `solana-instruction` `=3.2.0` → **`^3.4.0`**, agave-* → `^4.1.1`: the Agave-4.1-wave release the DB was waiting on. Conflicts rebounded `>=0.13` → `>=0.13, <0.14`, remedies flipped from downgrade-to-0.12 to upgrade-to-0.14 |
| — | mollusk-svm 0.14.0 (2026-07-08) dep metadata | drops `agave-syscalls`, rides `^4.1.1`. Same rebound for the three mollusk entries |
| c21 | arch v1/v2/v3 `.so` executed under litesvm 0.14.0 **and** 0.15.0 | **all three EXECUTE** (program body confirmed via `Program log:`; truncated + garbage ELF controls correctly `LOAD FAIL`). The 0.12/0.13 arch trap is GONE → new `litesvm_runtimes` rows `arch_ok = [1,2,3]` |
| c23 | `sondir fix --write` against a synthetic workspace on litesvm 0.13.1 + instructions-sysvar 3.0.1, then mollusk-svm 0.13.4 + the same | **both remedies hold end-to-end**: 0.13.1 -> 0.14 and 0.13.4 -> 0.14, each passing verify-then-keep (workspace re-resolves, no rollback). The flipped `fix_pin` direction is proven, not just asserted |
| c22 | bare `cargo add litesvm@0.14/0.15` with **no lockfile** | **COMPILE FAIL** (E0277): `solana-address 2.7.0` + `solana-message 4.4.0` moved to `wincode ^0.6.0` while `solana-instruction 3.4.0` (newest) still derives its schema impls from `wincode ^0.5.0`. Both wincode majors land in one graph and `Pubkey` fails the trait bound |

| c24 | `sondir sweep` (66 pairs, latest × latest) after the thaw | **66 probed, 66 clean, 0 hits** — and that is the finding, not the good news: see discovery 12 |

| c25 | bump ephemeral-rollups-sdk 0.15.5 -> 0.16.2 in a real workspace (raflux) | **blocked by the same split**: 0.16.2 pulls the wincode 0.6 wave in, and `cargo check --tests` fails with E0277 across four crates. Reverted. A routine minor bump, not a `cargo update`, is enough to trip it |

| c26 | upstream research on the wincode split (crates.io API + anza-xyz/solana-sdk commit log + GitHub issue search) | root cause found, **and one of our own claims refuted** — see discovery 14 |

| c27 | full freshness audit of every crate named in facts.toml against crates.io | found the mechanism the wincode entry had *upside down*, plus four bound defects — see discovery 15 |

### Discoveries

15. **The trap is beneath the crates you can see (c27).** The wincode entry named
    solana-instruction 3.4.0 as the 0.5-wave holdout everyone waits on. It is not a clean
    0.5 island: its NON-OPTIONAL `solana-instruction-error ^2.2.0` resolves to 2.5.0, which
    is on wincode 0.6. Same for the crates listed on the 0.5 side — `litesvm 0.15.0` hard-pins
    `wincode ^0.5.5` while its mandatory `solana-instruction-error ^2.4.0` and
    `solana-transaction-error ^3.3.1` float to the 0.6 releases. **That is the real reason a
    bare `cargo add litesvm@0.15` reproduces with nothing else present** — the earlier
    attribution to solana-address was incomplete. `mollusk-svm 0.14.0` drags the wave the same
    way with no direct wincode dep at all. Consequence: no upper bound on the visible crates
    can keep 0.6 out, which turns "don't pin, wait" from a preference into the only option.
    The wave is 13 crates, not 7 — `solana-hard-forks`, `solana-inflation` and
    `solana-reward-info` were also missing.

    **Second correction to c25.** "Bumping ephemeral-rollups-sdk 0.15.5 -> 0.16.2 pulls the
    0.6 wave in" is wrong about the cause. Diffing the two releases: the only change is
    `magicblock-delegation-program-api ^3.0.0 -> ^3.1.0`; no solana-* requirement moved. The
    bump was the *trigger* — it forced re-resolution — not the cause. Any manifest edit
    touching that subtree does it, which makes the hazard broader than recorded.

    **Bound defects fixed:** `ephemeral-rollups-sdk@*` made cargo enumerate all 76 published
    versions and fail with a feature error on ancient 0.0.x releases instead of the conflict
    (now `>=0.15`); `ephemeral-vrf-sdk >=0.3` named a range whose every member is yanked (now
    `>=0.4`); the litesvm sysvar entry discussed only 3.0.1 while 4.0.0 has shipped; and the
    litesvm-magicblock mechanism text had drifted — the probe still fails, but the resolver's
    first casualty is now `solana-loader-v4-program 4.0.0`, not `solana-instruction =3.2.0`.

    Tightening that probe also exposed a **latent bug in `doctor`**, not just a test failure:
    `present_matching` applied the named crate's version req to its EQUIVALENT crate too.
    ephemeral-rollups-sdk (0.16.x) and ephemeral-vrf-sdk (0.4.x) version independently, so any
    req tighter than `*` silently stopped matching the twin — detection would have been lost
    for every project that reaches MagicBlock through ephemeral-vrf-sdk. The wildcard had been
    hiding it since the entry was written. Equivalence now means presence; the req applies only
    to the crate that is named.

    One audit concern closed rather than acted on: `spl-token-interface 3.0.0` and
    `anchor-lang 1.1.2` sit on `solana-pubkey ^3.0.0`, behind the 4.x wave that light-sdk and
    mollusk require, and were flagged as untested pairings. They are not untested — sweep's
    66 pairs is exactly C(12,2) over the 12 ALIASES, so every such pair was probed on
    2026-07-27 and came back clean (c24).

16. **An Agave 4.2 wave is forming (c27, watch item).** `mollusk-svm 0.14.0-agave-4.2.0-beta.2`
    (2026-07-26) alongside `solana-program-runtime 4.2.0-rc.0` and `agave-feature-set 4.2.0-rc.0`
    (2026-07-24). The 0.13 -> 0.14 schism this DB documents is set to repeat at 0.14 -> 0.15.
    Nothing to record as a conflict yet — no stable release has landed — but the bounds added
    today (`>=0.13, <0.14`) are the right shape for it.

14. **The split has a name, an end date, and one of our claims was wrong (c26).**
    Root cause is [anza-xyz/solana-sdk#832](https://github.com/anza-xyz/solana-sdk/pull/832),
    merged 2026-07-21: a single `[workspace.dependencies] wincode` flip to 0.6.0. Master is
    uniformly on 0.6; only the leaf-first crates.io republish lags. So this is a publish wave
    mid-flight, not a stalled migration — and the precedent round (0.4→0.5, #647) took 7 days
    from bump to `solana-instruction`'s republish, putting this around 2026-07-28. The single
    unblocking release is **solana-instruction 3.5.0/3.4.1 on wincode ^0.6.0**. Not ours alone
    either: [LiteSVM/litesvm#386](https://github.com/LiteSVM/litesvm/issues/386) reports the
    identical E0277 (opened 2026-07-25, maintainer committed to a fix 2026-07-26).

    **Correction.** The entry listed `solana-message >=4.4` in the 0.6 wave. It is not — 4.4.0
    is on `wincode ^0.5.0`, published five hours *before* #832 merged. The error came from
    reading a `cargo update --dry-run` plan: solana-message 4.3.0 → 4.4.0 moved in the same
    batch that added wincode 0.6, and correlation got recorded as causation. Our own
    `cargo tree -i wincode@0.6.0` output had already excluded it. The real path is
    `Pubkey` → `solana-address::Address`, and address 2.7.0 is the crate that crossed.
    Fixed, along with the full membership of both sides.

13. **The split blocks upgrades, not just updates (c25).** The wincode entry was written
    as a `cargo update` hazard; it is broader. Any dependency bump that reaches the 0.6
    wave does it — ephemeral-rollups-sdk 0.16.2 is one. Also worth recording: the boundary
    is only crossed where both sides get compiled. `cargo check -p raflux-randomness`
    passed on the very graph whose `cargo check --tests` failed, so "two majors in the
    lockfile" is a risk signal, not proof — judge it on the build CI actually runs.

12. **The radar's blind spot, demonstrated live (c24).** Sweep reported the ecosystem
    entirely clean on the same day c22 proved a fresh `cargo add litesvm@0.15` cannot be
    COMPILED. Both statements are true: the wincode 0.5/0.6 split resolves perfectly, so
    every resolve-level probe walks past it. An all-clear from `sweep` currently means
    "no version-selection conflicts", NOT "a fresh project builds" — and nothing in the
    tool says so. This is the strongest argument yet for a compile-level fact category;
    until it exists, sweep's green is narrower than it reads.

9. **The arch trap inverted (c21).** Under litesvm ≥0.14 every arch executes, so a green
   `cargo test` no longer implies a deployable artifact — SIMD-0500 still bars v0/v1/v2 at
   the cluster. `arch-litesvm` used to catch that skew for free; on 4.1 runtimes only the
   `arch-cluster` check does. Recorded in the 0.14 runtime note.
10. **Resolve-clean, compile-broken (c22).** `wincode` 0.5/0.6 is the first conflict found
    that cargo's resolver *accepts* — it is a trait-coherence failure, not a version
    selection failure. `facts verify` (verify.rs: greps `failed to select`) and `sweep`
    both probe resolution only, so this class is invisible to the current fact model and
    was deliberately NOT written as a `[[conflicts]]` entry — a probe would report it
    "resolves" and mark it stale forever. Needs a compile-level fact category.
11. **Lockfiles are load-bearing right now.** `~/MonkLabs/raflux` builds litesvm 0.14.0 with
    130 green tests purely because its `Cargo.lock` holds `solana-address 2.6.1` /
    `solana-message 4.3.0` / `wincode 0.5.5`. `cargo update --dry-run` there shows the move
    to 2.7.0 / 4.4.0 and **`Adding wincode v0.6.0`** — a plain `cargo update` breaks it.
