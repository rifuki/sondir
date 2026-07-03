# litesvm upstream issue — FILED

> Status: FILED 2026-07-03 as https://github.com/LiteSVM/litesvm/issues/372
> (owner said "gas dah"). Text below is the source; the issue itself omits the
> DRAFT header. Can be edited/closed on the issue page.

**Title:** `0.13.x pins `solana-instruction = "=3.2.0"`, making it unresolvable with the `^3.4` interface wave (MagicBlock VRF, `solana-instructions-sysvar` 3.0.1+)`

---

**litesvm version:** 0.13.1
**Anchor:** 1.1.2 (its `anchor init` template already pins `litesvm = "0.13.1"`)
**Platform:** doesn't matter — this is a `cargo` resolution failure, no build/run needed.

### Summary

`litesvm` 0.13.x carries an **exact** requirement `solana-instruction = "=3.2.0"` (the Agave
4.0 interface wave). Any workspace that also pulls a crate on the **Agave 4.1** interface wave —
which requires `solana-instruction ^3.4` — cannot resolve, because cargo will not unify an
exact `=3.2.0` with a caret `^3.4` inside the same major. The two most common triggers:

- **MagicBlock** `ephemeral-rollups-sdk` (with the `vrf` feature) / `ephemeral-vrf-sdk` ≥ 0.3
- **`solana-instructions-sysvar`** ≥ 3.0.1 (3.0.1 bumped its req to `solana-instruction ^3.4.0`)

Because `anchor init` on the current CLI (1.1.2) templates `litesvm = "0.13.1"`, a brand-new
Anchor project that adds MagicBlock VRF hits this immediately, out of the box.

### Reproduce

```toml
# Cargo.toml
[dependencies]
litesvm = "0.13"
ephemeral-rollups-sdk = { version = "*", features = ["anchor", "vrf"] }
```

```
$ cargo generate-lockfile
error: failed to select a version for `solana-instruction`.
    ... required by package `litesvm v0.13.1`   (=3.2.0)
    ... required by package `ephemeral-vrf-sdk` (^3.4)
```

Same failure with `solana-instructions-sysvar = "3.0.1"` in place of the MagicBlock line.

### Why 0.12.x works but 0.13.x doesn't

0.12.x (Agave 3.1.14 runtime) carries **loose** `^3.x`-style reqs, so it unifies with the 4.1
wave fine. 0.13.x inherited the `=3.2.0` exact pin from its Agave 4.0 bump. So users are stuck
on 0.12.x specifically to keep MagicBlock/instructions-sysvar resolvable — which also forces
their test `.so` to be built `--arch v1` (0.12's runtime is pre-SBPFv3), an awkward split.

### Ask

Either of:

1. **Relax the pin** to a caret (`solana-instruction = "^3.2"` or wider) if 0.13's runtime is
   actually compatible with the 3.4 interface types — this alone unblocks everyone; or
2. **Ship an Agave-4.1-wave release** of litesvm (`solana-instruction ^3.4`, SBPFv3 runtime),
   which would let the whole ecosystem move to one interface wave and retire the arch split.

### Evidence

Verified empirically across a 20-project canary matrix (2026-07-02 → 07-03): c05 (fresh
`anchor init` + MagicBlock), c06 (`instructions-sysvar` 3.0.1), and the 0.12-vs-0.13 arch split
(c15 confirmed 0.12 runs `--arch v1`/v2 only). Happy to share the matrix.
