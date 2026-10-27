#!/usr/bin/env bash
# Headless two-process simulation. Not a hardware-channel result.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release/cpu-cache-chat}"
if [[ ! -x "$BIN" ]]; then
  cargo build -p cpu-cache-chat --release
  BIN="$ROOT/target/release/cpu-cache-chat"
fi
MODE="${MODE:-chat}"
WORKDIR="${TMPDIR:-/tmp}/ccc-e2e-$$"
mkdir -p "$WORKDIR"
SIM="$WORKDIR/link.sim"
A_IN="$WORKDIR/a.in"
B_IN="$WORKDIR/b.in"
A_OUT="$WORKDIR/a.out"
B_OUT="$WORKDIR/b.out"
mkfifo "$A_IN" "$B_IN"
cleanup() {
  kill "$A_PID" "$B_PID" 2>/dev/null || true
  wait "$A_PID" "$B_PID" 2>/dev/null || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

: >"$A_OUT"
: >"$B_OUT"
"$BIN" "$MODE" --role a --channel cache --sim --sim-path "$SIM" --plain <"$A_IN" >"$A_OUT" 2>&1 &
A_PID=$!
"$BIN" "$MODE" --role b --channel cache --sim --sim-path "$SIM" --plain <"$B_IN" >"$B_OUT" 2>&1 &
B_PID=$!
exec 3>"$A_IN"
exec 4>"$B_IN"

wait_for() {
  local file="$1" needle="$2" tries="${3:-80}"
  for _ in $(seq 1 "$tries"); do
    if grep -q -- "$needle" "$file"; then
      return 0
    fi
    sleep 0.1
  done
  echo "timeout waiting for '$needle' in $file" >&2
  echo "----- $(basename "$file") -----" >&2
  cat "$file" >&2 || true
  return 1
}

wait_for "$A_OUT" "link: connected" 100
wait_for "$B_OUT" "link: connected" 100

if [[ "$MODE" == chat ]]; then
  printf 'ping-from-a\n' >&3
  wait_for "$A_OUT" "seq 1: delivered" 80
  wait_for "$B_OUT" "peer: ping-from-a" 80
  printf 'pong-from-b\n' >&4
  wait_for "$B_OUT" "seq 1: delivered" 80
  wait_for "$A_OUT" "peer: pong-from-b" 80
  echo "PASS chat simulation both directions"
else
  printf '5\n' >&3
  wait_for "$A_OUT" "seq 1: delivered" 80
  wait_for "$B_OUT" "X" 80
  printf '1\n' >&4
  wait_for "$B_OUT" "seq 1: delivered" 80
  wait_for "$A_OUT" "O" 80
  echo "PASS game simulation both directions"
fi

printf '/q\n' >&3
printf '/q\n' >&4
exec 3>&-
exec 4>&-
wait "$A_PID" "$B_PID" || true
