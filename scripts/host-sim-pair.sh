#!/usr/bin/env bash
# Two local processes over the labeled simulation transport.
# Not the isolated hardware demo.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/debug/cpu-cache-chat}"
if [[ ! -x "$BIN" ]]; then
  cargo build -p cpu-cache-chat
  BIN="$ROOT/target/debug/cpu-cache-chat"
fi
PATH_BASE="${SIM_PATH:-/tmp/cpu-cache-chat.sim}"
MODE="${MODE:-chat}"
echo "SIMULATION pair. mode=${MODE} path=${PATH_BASE}"
"$BIN" "$MODE" --role a --channel cache --sim --sim-path "$PATH_BASE" --plain &
pid_a=$!
"$BIN" "$MODE" --role b --channel cache --sim --sim-path "$PATH_BASE" --plain &
pid_b=$!
trap 'kill $pid_a $pid_b 2>/dev/null || true' EXIT
wait
