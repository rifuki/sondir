#!/usr/bin/env bash
# sondir canary runner — creates intentionally conflict-prone Anchor projects and
# records how far each one gets: resolve -> native check -> (optional) SBF build
# -> sondir doctor. Results append to canary/results.md.
#
# Usage: bash canary/run.sh <id>            (e.g. bash canary/run.sh c05)
# Env:   CANARY_DIR (default ~/MonkLabs/sondir-canary)
#        SONDIR_BIN (default <repo>/target/release/sondir)
set -uo pipefail

ID="${1:?usage: run.sh <cNN>}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CANARY_DIR="${CANARY_DIR:-$HOME/MonkLabs/sondir-canary}"
SONDIR_BIN="${SONDIR_BIN:-$REPO_ROOT/target/release/sondir}"
RESULTS="$SCRIPT_DIR/results.md"
SHARED_TARGET="$CANARY_DIR/_native-target"
mkdir -p "$CANARY_DIR"

PROJ="$CANARY_DIR/$ID"
MANIFEST="$PROJ/programs/$ID/Cargo.toml"

note() { printf '%s\n' "$*"; }

add() { cargo add --quiet --manifest-path "$MANIFEST" "$@" 2>&1 | tail -2; }

init_project() {
  if [ -d "$PROJ" ]; then return 0; fi
  (cd "$CANARY_DIR" && NO_DNA=1 anchor init "$ID" --no-install >/dev/null 2>&1)
}

mutate() {
  case "$ID" in
    c01) note "vanilla anchor init (baseline)";;
    c02) add --dev litesvm@0.12.0;;
    c03) add --dev litesvm@0.13.1;;
    c04) add ephemeral-rollups-sdk@0.15.5 --features anchor,vrf;;
    c05) add ephemeral-rollups-sdk@0.15.5 --features anchor,vrf; add --dev litesvm@0.13.1;;
    c06) add --dev litesvm@0.13.1; add solana-instructions-sysvar@3.0.1;;
    c07) add --dev litesvm@0.13.1; add solana-instructions-sysvar@=3.0.0;;
    c08) add anchor-spl@1.1.2 --no-default-features --features token,token_2022;;
    c09) add spl-token-interface@3.0.0;;
    c10) add blake3;;
    c11) add light-sdk;;
    c12) add pyth-solana-receiver-sdk;;
    c13) add switchboard-on-demand;;
    c14) add mpl-core;;
    c15) note "arch v1 deploy probe (mutation: none; built with --arch v1)";;
    c16) note "arch v3 deploy probe (mutation: none; built with --arch v3)";;
    c17) note "grow-under-10240 upgrade trap (two-stage manual deploy)";;
    c18) (cd "$PROJ" && NO_DNA=1 anchor new second_program >/dev/null 2>&1) && note "added second_program";;
    c19) add solana-program@1.18;;
    c20) add anchor-lang@0.31.1;;
    *) note "unknown canary id $ID"; exit 2;;
  esac
}

main() {
  init_project
  mutate

  local resolve="OK" check="-" note_txt=""
  local resolve_err
  resolve_err="$( (cd "$PROJ" && cargo metadata --format-version 1 >/dev/null) 2>&1 )" || resolve="FAIL"
  if [ "$resolve" = "FAIL" ]; then
    note_txt="$(printf '%s' "$resolve_err" | grep -E 'failed to select|conflict|required by' | head -3 | tr '\n' ' ' | cut -c1-220)"
  else
    if (cd "$PROJ" && CARGO_TARGET_DIR="$SHARED_TARGET" cargo check -q >/dev/null 2>"$CANARY_DIR/$ID.check.err"); then
      check="OK"
    else
      check="FAIL"
      note_txt="$(grep -E '^error' "$CANARY_DIR/$ID.check.err" | head -2 | tr '\n' ' ' | cut -c1-220)"
    fi
  fi

  local doctor="-"
  if [ -x "$SONDIR_BIN" ]; then
    local json
    json="$("$SONDIR_BIN" doctor --path "$PROJ" --offline --json 2>/dev/null)" || true
    if [ -n "$json" ]; then
      doctor="$(printf '%s' "$json" | jq -r '[.findings[] | select(.severity=="fail")] | length') fail / $(printf '%s' "$json" | jq -r '[.findings[] | select(.severity=="warn")] | length') warn"
    fi
  fi

  printf '| %s | %s | %s | %s | %s |\n' "$ID" "$resolve" "$check" "$doctor" "${note_txt:-—}" >> "$RESULTS"
  note "[$ID] resolve=$resolve check=$check doctor=($doctor) ${note_txt:+· $note_txt}"
}

main
