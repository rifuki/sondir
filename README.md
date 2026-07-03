# sondir

> **sondir** (Indonesian, from Dutch *sonderen*): the soil penetration test you run
> **before you build** — so the foundation doesn't surprise you later.

Pre-flight checks and version compatibility for Solana toolchains. `sondir doctor` turns
deploy-time surprises into actionable warnings *before* a transaction is sent — read-only,
no compile, no signature.

Born from a real incident chain (2026-07-02): a bare `anchor test` silently upgraded a live
devnet program with the wrong SBPF arch, an upgrade stranded 6.2 SOL in a buffer because
SIMD-0431 rejected anchor's auto-extend, and an on-chain IDL write failed with an opaque
error because the metadata account already existed. Every one of those was knowable up
front from local files + RPC reads. `sondir` knows them now.

## Usage

```bash
sondir doctor [--path <anchor-workspace>] [--url <rpc>] [--json] [--offline]
```

- `--url` (or `$SONDIR_RPC`) overrides the RPC; defaults from Anchor.toml
  `provider.cluster`. Use a dedicated RPC — public devnet rate-limits.
- `--json` for agents/CI. Exit codes: 0 clean · 1 any FAIL finding · 2 execution error.
- Output is ANSI-free automatically when piped (CI logs stay clean).
- `--offline` skips RPC (local checks only).

## Checks (v0.2)

| Code | Catches |
|---|---|
| `resolve` | the workspace does not resolve at all (probes cargo itself — the lockfile can lie after a failed `cargo add`) |
| `toolchain-anchor` | anchor CLI vs anchor-lang crate mismatch (+ `[toolchain]` pin advice) |
| `dep-conflict` | known-unresolvable pairs: litesvm 0.13.x × MagicBlock vrf, litesvm 0.13.x × instructions-sysvar ≥3.0.1, legacy solana-program 1.x × modern workspace (all canary-verified) |
| `keypair-drift` | `target/deploy/*-keypair.json` ≠ Anchor.toml program id — `anchor deploy` targets the keypair and silently lands on the wrong address |
| `gate` | live feature-gate status: SBPFv3, SIMD-0431 (min-extend 10240), SIMD-0500 (v0–v2 deploy ban) |
| `arch-cluster` | `.so` SBPF arch flag (ELF e_flags, byte 48) vs what the target cluster accepts |
| `arch-litesvm` | `.so` arch vs what the locked litesvm runtime executes ("Access violation" prevention) |
| `simd0431-extend` | upgrade will fail because the binary grew and extend rules changed; exact `solana program extend` fix |
| `upgrade-authority` | configured wallet is not the on-chain upgrade authority |
| `stranded-buffer` | leftover `*-upgrade-buffer.json` with rent still locked on-chain; resume/close command |
| `balance` | wallet can't afford the largest upgrade buffer |
| `anchor-test-footgun` | non-local provider.cluster: bare `anchor test` deploys before testing |
| `idl-rule` | IDL metadata init-once/upgrade-after rule (the opaque "transaction plan failed" error) |

## Why not just read the error message?

Because the error arrives *after* the damage: SIMD-0431 rejects the extend **after** the
6-SOL buffer is written; the wrong-arch `.so` fails **after** it replaced the right one
on-chain; the IDL error names neither the metadata account nor the fix. Every check above
is derived from a failure that actually happened and is fully predictable from
`Cargo.lock` + `Anchor.toml` + `target/deploy` + a handful of RPC reads.

## Canary matrix

`canary/` holds a matrix of intentionally conflict-prone Anchor projects used to validate
sondir's facts empirically (resolve → build → test → devnet deploy → upgrade). Results in
`canary/results.md`; each confirmed behavior feeds back into `src/facts.rs`.

## Roadmap

- `resolve` — multi-select ecosystem deps (litesvm, MagicBlock, Light Protocol, Metaplex, …)
  → synthetic manifest → cargo resolver → explained compatible version set.
- Facts DB externalized (rustsec-style: every entry carries evidence + verified date).
- `watch` — notify when an upstream release unlocks held-back upgrades
  (e.g. litesvm's Agave-4.1 wave).
- MCP server exposing `doctor`/`resolve` to AI agents.

## Design notes

- Read-only by construction: sondir never sends a transaction.
- Shells out to `solana-keygen pubkey` / `anchor --version` instead of pulling ed25519 or
  CLI internals into the dependency tree.
- Facts live in `src/facts.rs`; each entry states its consequence so checks stay declarative.

## License

MIT
