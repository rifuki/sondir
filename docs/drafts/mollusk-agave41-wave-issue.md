# DRAFT — mollusk upstream issue (NOT FILED)

> Status: draft, awaiting owner go-ahead before filing at
> https://github.com/anza-xyz/mollusk/issues . Nothing has been posted.
> Companion to LiteSVM/litesvm#372 (maintainer there confirmed an Agave-4.1-wave
> release is coming); also draft a cross-reference comment on #372 (below).

**Title:** `0.13.x (via agave-syscalls 4.0.0) pins the Agave-4.0 interface wave — unresolvable with the ^3.4/4.x ecosystem (MagicBlock VRF, light-sdk 0.24, solana-instructions-sysvar ≥3.0.1)`

---

**mollusk-svm version:** 0.13.4

### Summary

`mollusk-svm` 0.13.x depends on `agave-syscalls 4.0.0`, which carries exact/tight pins on the
**Agave 4.0** interface wave (`solana-instruction`, `solana-pubkey`). Any workspace that also
pulls a crate on the **Agave 4.1** interface wave (`solana-instruction ^3.4` /
`solana-pubkey 4.x`) cannot resolve. Reproduced triggers:

- **MagicBlock** `ephemeral-rollups-sdk` (with `vrf`) / `ephemeral-vrf-sdk` ≥ 0.3
- **`light-sdk`** ≥ 0.24 (Light Protocol)
- **`solana-instructions-sysvar`** ≥ 3.0.1

This is the same shape as LiteSVM/litesvm#372 (litesvm 0.13.x, `solana-instruction =3.2.0`),
where the maintainer confirmed a 4.1-wave release is planned — i.e. the schism is
ecosystem-wide across the SVM test harnesses.

### Reproduce

```toml
[dependencies]
mollusk-svm = "=0.13.4"
ephemeral-rollups-sdk = { version = "*", features = ["anchor", "vrf"] }
```

```
$ cargo generate-lockfile
error: failed to select a version for `solana-instruction`.
    ... required by package `agave-syscalls v4.0.0`
```

Same failure with `light-sdk = "0.24"` (on `solana-pubkey`) or
`solana-instructions-sysvar = "4.0.0"` in place of the MagicBlock line.

### Current escape

Downgrading to `mollusk-svm 0.12.x` resolves in all three cases (verified via cargo's own
resolver). So users are pinned one minor back, mirroring the litesvm situation.

### Ask

A release riding the Agave-4.1 interface wave (agave-syscalls with `solana-instruction ^3.4`
/ `solana-pubkey 4.x` reqs), or relaxed pins if the runtime is compatible.

### Evidence

Found by an automated pairwise resolver sweep across Solana ecosystem crates (2026-07-04),
each case confirmed by an isolated 2-crate `cargo generate-lockfile` probe. Happy to share
the probe manifests.

---

# DRAFT — cross-reference comment for LiteSVM/litesvm#372 (NOT POSTED)

> For context on the issue we already filed. Post only with owner go-ahead.

FWIW, this turns out to be ecosystem-wide across the SVM test harnesses: `mollusk-svm`
0.13.x has the same shape via its `agave-syscalls 4.0.0` pins (breaks against MagicBlock
VRF, `light-sdk` 0.24 and `solana-instructions-sysvar` ≥3.0.1; `0.12.x` resolves) —
reported at anza-xyz/mollusk#<N>. A 4.1-wave litesvm release would resolve the largest
share of these, thanks for picking it up.
