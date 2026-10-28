#!/usr/bin/env bash
# Print two independent terminal commands. No shared/competing background stdin.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/debug/cpu-cache-chat}"
MODE="${MODE:-chat}"
[[ "$MODE" == chat || "$MODE" == game ]] || { echo 'MODE must be chat or game' >&2; exit 1; }
if [[ ! -x "$BIN" ]]; then
  cargo build --locked --manifest-path "$ROOT/Cargo.toml" -p cpu-cache-chat
fi
WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/ccc-sim.XXXXXX")
printf 'SIMULATION only; not a physical channel. Use separate terminals:\n'
printf '  %q %q --role a --channel cpu --sim --sim-path %q --plain\n' "$BIN" "$MODE" "$WORKDIR/link.sim"
printf '  %q %q --role b --channel cpu --sim --sim-path %q --plain\n' "$BIN" "$MODE" "$WORKDIR/link.sim"
printf 'Enter /q in both terminals, then remove the empty private directory:\n  rmdir %q\n' "$WORKDIR"
