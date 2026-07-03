# DRAFT — go-public checklist (NOT EXECUTED)

> Status: draft, awaiting owner decision. None of these steps have run. Making
> the repo public and publishing to crates.io are one-way, outward-facing
> actions — do them only on explicit go-ahead.

## Before flipping the repo public

- [ ] **Scrub history for secrets.** No RPC URLs, keypairs, or API keys anywhere in the tree
      or git history. `git log -p | grep -iE 'quiknode|helius|api-key|-----BEGIN'` should be
      empty. (sondir has never contained the QuickNode devnet URL — keep it that way; it lives
      only in the raflux memory, never in this repo.)
- [ ] **LICENSE present** — MIT, already committed. ✅
- [ ] **README states scope + non-goals** — read-only, no-signature by construction. ✅
- [ ] **`facts.toml` claims all cite evidence** (canary id + date) so third parties can verify
      or challenge them. ✅ (already the invariant)
- [ ] Decide the **facts DB home**: keep embedded, or split into a companion `sondir-facts`
      repo (rustsec-advisory-db style) that the binary vendors. Public contributors can PR
      facts without touching the binary. (Roadmap item — not required for v1.)
- [ ] CI green on `main` (fmt + clippy -D + test). ✅ as of v0.3.0

## crates.io publish (`sondir`)

- [ ] Confirm the name `sondir` is free on crates.io (`cargo search sondir`).
- [ ] `Cargo.toml` metadata complete: `description`, `license`, `repository`, `keywords`
      (`solana`, `anchor`, `preflight`, `mcp`), `categories` (`command-line-utilities`,
      `development-tools`). Currently has description + license only — **add repository +
      keywords + categories before publish.**
- [ ] `cargo publish --dry-run` clean.
- [ ] Tag already matches (`v0.3.0`). Release workflow builds macOS aarch64 + Linux x86_64
      tarballs on tag. ✅
- [ ] `cargo publish` (irreversible — a version can be yanked but never re-used).

## Announce (optional, after the above)

- [ ] File the litesvm issue (see `litesvm-agave41-wave-issue.md`) — this is the natural
      "why does this tool exist" hook.
- [ ] Short write-up: the incident chain → what each check prevents.

## Owner decisions still open

1. Repo public now, or keep private until crates.io metadata + facts-repo split are done?
2. Publish under `rifuki` or a MonkLabs org crate owner?
3. File the litesvm issue before or after going public (issue links back to the repo)?
