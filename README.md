# solana-compat

Solana toolchain compatibility helper. First shipped command: **`doctor`** — read-only
pre-flight checks that turn deploy-time surprises into actionable warnings *before* a
transaction is sent.

Born from a real incident chain (2026-07-02): a bare `anchor test` silently upgraded a live
devnet program with the wrong SBPF arch, an upgrade stranded 6.2 SOL in a buffer because
SIMD-0431 rejected anchor's auto-extend, and an on-chain IDL write failed with an opaque
error because the metadata account already existed. Every one of those was knowable
up front from local files + RPC reads. `doctor` knows them now.

## Usage

```bash
solana-compat doctor [--path <anchor-workspace>] [--url <rpc>] [--json] [--offline]
```

- `--url` (or `$SOLANA_COMPAT_RPC`) overrides the RPC; defaults from Anchor.toml
  `provider.cluster`. Use a dedicated RPC — public devnet rate-limits.
- `--json` for agents/CI. Exit code 1 when any FAIL finding exists.
- `--offline` skips RPC (local checks only).

## Checks (v0.1)

| Code | Catches |
|---|---|
| `toolchain-anchor` | anchor CLI vs anchor-lang crate mismatch (+ `[toolchain]` pin advice) |
| `dep-conflict` | known-unresolvable pairs, e.g. litesvm 0.13.x × MagicBlock vrf (instruction =3.2.0 vs ^3.4) |
| `keypair-drift` | `target/deploy/*-keypair.json` ≠ Anchor.toml program id — `anchor deploy` targets the keypair, silently landing on the wrong address |
| `gate` | live feature-gate status: SBPFv3, SIMD-0431 (min-extend 10240), SIMD-0500 (v0–v2 deploy ban) |
| `arch-cluster` | `.so` SBPF arch flag (byte 48) vs what the target cluster accepts |
| `arch-litesvm` | `.so` arch vs what the locked litesvm runtime can execute ("Access violation" prevention) |
| `simd0431-extend` | upgrade will fail because the binary grew and the extend rules changed; exact `solana program extend` fix |
| `upgrade-authority` | configured wallet is not the on-chain upgrade authority |
| `stranded-buffer` | leftover `*-upgrade-buffer.json` with rent still locked on-chain; resume/close command |
| `balance` | wallet can't afford the largest upgrade buffer |
| `anchor-test-footgun` | non-local provider.cluster: bare `anchor test` deploys before testing |
| `idl-rule` | IDL metadata init-once/upgrade-after rule (the opaque "transaction plan failed" error) |

## Roadmap

- `resolve` — multi-select ecosystem deps (litesvm, MagicBlock, Light Protocol, Metaplex, …)
  → synthetic manifest → cargo resolver → explained compatible version set.
- Facts DB externalized to a community repo (rustsec-style: every entry has evidence + date).
- Canary CI: continuously build/test/deploy template combos on devnet → verification stamps.
- `watch` — notify when an upstream release unlocks held-back upgrades
  (e.g. litesvm's Agave-4.1 wave).
- MCP server exposing `resolve`/`doctor` to AI agents.

## Design notes

- Read-only by construction: doctor never sends a transaction.
- Shells out to `solana-keygen pubkey` / `anchor --version` instead of pulling ed25519/CLI
  internals into the dependency tree.
- Facts live in `src/facts.rs` for now; each entry states its consequence so checks stay
  declarative.
