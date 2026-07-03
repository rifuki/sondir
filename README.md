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
# pre-flight an Anchor workspace
sondir doctor [--path <anchor-workspace>] [--url <rpc>] [--json] [--offline]

# find a mutually-compatible version set for the deps you want
sondir resolve anchor litesvm magicblock        # -> litesvm 0.12.0 + why, in seconds
sondir resolve --list                           # known aliases (raw crate names work too)

# has an upstream release/gate unlocked a held-back upgrade yet? (cron/CI-friendly)
sondir watch [--url <rpc>] [--json]             # exit 3 when a trigger fired

# run as an MCP server so an AI agent can call doctor/resolve/watch
sondir mcp                                       # stdio, newline-delimited JSON-RPC

# any command accepts a custom facts database
sondir --facts my-facts.toml doctor ...
```

- `--url` (or `$SONDIR_RPC`) overrides the RPC; defaults from Anchor.toml
  `provider.cluster`. Use a dedicated RPC — public devnet rate-limits.
- `--json` for agents/CI. Exit codes: 0 clean · 1 any FAIL finding · 2 execution error.
- Output is ANSI-free automatically when piped (CI logs stay clean).
- `--offline` skips RPC (local checks only).

## doctor checks

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

## resolve

`resolve` synthesizes a throwaway manifest with your selection at `*` and lets cargo's own
resolver do the search, then reads the answer from the lockfile: exact versions, "pivot"
crates that reveal which Agave interface wave you landed on, and runtime notes (e.g. which
SBPF arch your litesvm needs). When resolution fails it retries with facts-driven remedies
("pin litesvm <0.13") so the answer is *works if you pin X*, not just *conflict*.

## Facts database

The knowledge that no Cargo metadata expresses lives in `facts/facts.toml` (feature gates
with consequences, known conflicts with evidence + fixes, litesvm runtime arch tables).
It ships embedded in the binary; override with `--facts <path>`. Every entry cites its
evidence (canary id + date) so it can be re-verified or retired.

## Canary matrix

`canary/` holds a matrix of intentionally conflict-prone Anchor projects used to validate
sondir's facts empirically (resolve → build → test → devnet deploy → upgrade). Results in
`canary/results.md`; each confirmed behavior feeds back into `src/facts.rs`.

## watch

`watch` checks whether an upstream event has unlocked an upgrade the canary campaign
told us to hold back: litesvm shipping its Agave-4.1 wave (crates.io — `solana-instruction`
requirement leaves the `=3.2.0` pin), SIMD-0500 activating on the target cluster (v0–v2
deploys die), or anchor crossing to the pubkey-4 interface wave. Each trigger prints
`FIRED`/`waiting` with what to do when it fires. Exit code `3` when anything fired, so a cron
or CI job can alert on it (`--json` for machine parsing).

## MCP server

`sondir mcp` speaks the Model Context Protocol over stdio (newline-delimited JSON-RPC 2.0),
exposing three tools to an AI agent: `sondir_doctor`, `sondir_resolve`, `sondir_watch`. Each
reuses the exact same code path as the CLI, so an agent gets identical results. Point any MCP
client at the `sondir mcp` command — no network server, no ports.

## Roadmap

- ~~`resolve` — multi-select ecosystem deps → cargo resolver → explained compatible set.~~ ✅
- ~~Facts DB externalized (rustsec-style: every entry carries evidence + verified date).~~ ✅
- ~~`watch` — notify when an upstream release/gate unlocks held-back upgrades.~~ ✅
- ~~MCP server exposing `doctor`/`resolve`/`watch` to AI agents.~~ ✅
- Publish to crates.io / make the repo public (pending owner decision).

## Design notes

- Read-only by construction: sondir never sends a transaction.
- Shells out to `solana-keygen pubkey` / `anchor --version` instead of pulling ed25519 or
  CLI internals into the dependency tree.
- Facts live in `src/facts.rs`; each entry states its consequence so checks stay declarative.

## License

MIT
