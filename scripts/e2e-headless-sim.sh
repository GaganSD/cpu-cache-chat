#!/usr/bin/env bash
# Headless two-process simulation. Not a hardware-channel result.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release/cpu-cache-chat}"
if [[ ! -x "$BIN" ]]; then
  cargo build --locked -p cpu-cache-chat -p ccc-game --release
  BIN="$ROOT/target/release/cpu-cache-chat"
fi
MODE="${MODE:-chat}"
WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/ccc-e2e.XXXXXX")
EVENT_A=()
EVENT_B=()
if [[ "$MODE" == game ]]; then
  EVENT_A=(--events-path "$WORKDIR/a.jsonl")
  EVENT_B=(--events-path "$WORKDIR/b.jsonl")
fi
SIM="$WORKDIR/link.sim"
A_IN="$WORKDIR/a.in"
B_IN="$WORKDIR/b.in"
A_OUT="$WORKDIR/a.out"
B_OUT="$WORKDIR/b.out"
mkfifo "$A_IN" "$B_IN"
cleanup() {
  local status=$?
  trap - EXIT
  kill "${A_PID:-}" "${B_PID:-}" 2>/dev/null || true
  wait "${A_PID:-}" "${B_PID:-}" 2>/dev/null || true
  if [[ "$status" == 0 && -z "${RETAIN:-}" ]]; then
    rm -rf "$WORKDIR"
  else
    echo "Simulation diagnostics retained: $WORKDIR" >&2
  fi
  exit "$status"
}
trap cleanup EXIT

: >"$A_OUT"
: >"$B_OUT"
"$BIN" "$MODE" --role a --channel cache --sim --sim-path "$SIM" --plain ${EVENT_A[@]+"${EVENT_A[@]}"} <"$A_IN" >"$A_OUT" 2>&1 &
A_PID=$!
"$BIN" "$MODE" --role b --channel cache --sim --sim-path "$SIM" --plain ${EVENT_B[@]+"${EVENT_B[@]}"} <"$B_IN" >"$B_OUT" 2>&1 &
B_PID=$!
exec 3>"$A_IN"
exec 4>"$B_IN"

wait_for() {
  local file="$1" needle="$2" tries="${3:-80}"
  for _ in $(seq 1 "$tries"); do
    if grep -Fq -- "$needle" "$file"; then
      return 0
    fi
    sleep 0.1
  done
  echo "timeout waiting for '$needle' in $file" >&2
  echo "----- $(basename "$file") -----" >&2
  cat "$file" >&2 || true
  return 1
}

wait_for "$A_OUT" "mode: SIMULATION (software only)" 100
wait_for "$B_OUT" "mode: SIMULATION (software only)" 100
wait_for "$A_OUT" "link: connected" 100
wait_for "$B_OUT" "link: connected" 100

if [[ "$MODE" == chat ]]; then
  printf 'ping-from-a\n' >&3
  wait_for "$A_OUT" "seq 1: delivered" 80
  wait_for "$B_OUT" "peer: ping-from-a" 80
  printf 'pong-from-b\n' >&4
  wait_for "$B_OUT" "seq 1: delivered" 80
  wait_for "$A_OUT" "peer: pong-from-b" 80
  printf 'control-\033[31m-text\n' >&3
  wait_for "$B_OUT" 'peer: control-\u{1b}[31m-text' 80
  if LC_ALL=C grep -q "$(printf '\033')" "$B_OUT"; then
    echo "FAIL: unescaped terminal control in peer output" >&2
    exit 1
  fi
  echo "PASS chat simulation both directions; terminal control escaped"
else
  wait_for "$A_OUT" "seq 1: delivered" 80 # Start, not a move.
  printf '5\n' >&3
  wait_for "$A_OUT" "seq 2: delivered" 80
  wait_for "$B_OUT" " 4 | X | 6" 80
  printf '2\n' >&4
  wait_for "$B_OUT" "seq 1: delivered" 80
  wait_for "$A_OUT" " 1 | O | 3" 80
  printf '1\n' >&3
  wait_for "$B_OUT" 'peer move 1 (ply 2)' 80
  printf '8\n' >&4
  wait_for "$A_OUT" 'peer move 8 (ply 3)' 80
  printf '9\n' >&3
  wait_for "$A_OUT" 'X wins' 80
  wait_for "$B_OUT" 'X wins' 80
  for output in "$A_OUT" "$B_OUT"; do
    wait_for "$output" ' X | O | 3' 80
    wait_for "$output" ' 7 | O | X' 80
  done
  echo "PASS complete game simulation on both boards"
fi

printf '/q\n' >&3
printf '/q\n' >&4
exec 3>&-
exec 4>&-
status=0
wait "$A_PID" || status=1
wait "$B_PID" || status=1
if [[ "$status" != 0 ]]; then
  echo 'FAIL: a simulation process exited unsuccessfully' >&2
  exit 1
fi
if [[ "$MODE" == game ]]; then
  VERIFY="${VERIFY:-$(dirname "$BIN")/ccc-verify-game}"
  "$VERIFY" "$WORKDIR/a.jsonl" "$WORKDIR/b.jsonl"
fi
