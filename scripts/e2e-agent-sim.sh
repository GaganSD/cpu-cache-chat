#!/usr/bin/env bash
# Agent-versus-agent game over the SIMULATION transport on this host.
# This validates the Pi agent tool loop and worker integration only; it is not
# a hardware-channel result. Requires OPENCODE_GO_API_KEY in the environment
# when CCC_AGENT_PROVIDER=opencode-go (the default here).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release/cpu-cache-chat}"
if [[ ! -x "$BIN" ]]; then
  cargo build -p cpu-cache-chat --release
  BIN="$ROOT/target/release/cpu-cache-chat"
fi
export CCC_AGENT_PROVIDER="${CCC_AGENT_PROVIDER:-opencode-go}"
if [[ "$CCC_AGENT_PROVIDER" == opencode-go && -z "${OPENCODE_GO_API_KEY:-}" ]]; then
  echo 'OPENCODE_GO_API_KEY must be set for CCC_AGENT_PROVIDER=opencode-go' >&2
  exit 1
fi
WAIT_SECONDS="${WAIT_SECONDS:-600}"
WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/ccc-agent-sim.XXXXXX")
cleanup() {
  kill "${A_PID:-}" "${B_PID:-}" 2>/dev/null || true
  wait "${A_PID:-}" "${B_PID:-}" 2>/dev/null || true
  if [[ -z "${RETAIN:-}" ]]; then rm -rf "$WORKDIR"; else echo "evidence: $WORKDIR"; fi
}
trap cleanup EXIT
mkfifo "$WORKDIR/a.in" "$WORKDIR/b.in"
: >"$WORKDIR/a.out"
: >"$WORKDIR/b.out"
"$BIN" game --role a --channel cpu --sim --sim-path "$WORKDIR/link.sim" --plain \
  --agent --agent-cmd "node $ROOT/agent/src/play.mjs" \
  <"$WORKDIR/a.in" >"$WORKDIR/a.out" 2>&1 &
A_PID=$!
"$BIN" game --role b --channel cpu --sim --sim-path "$WORKDIR/link.sim" --plain \
  --agent --agent-cmd "node $ROOT/agent/src/play.mjs" \
  <"$WORKDIR/b.in" >"$WORKDIR/b.out" 2>&1 &
B_PID=$!
exec 3>"$WORKDIR/a.in" 4>"$WORKDIR/b.in"
wait_for() {
  local file="$1" needle="$2" tries="${3:-$((WAIT_SECONDS))}"
  for _ in $(seq 1 "$tries"); do
    if grep -Fq -- "$needle" "$file"; then return 0; fi
    if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
    sleep 1
  done
  echo "FAIL: waiting for '$needle' in $file" >&2
  tail -20 "$file" >&2 || true
  return 1
}
wait_for "$WORKDIR/a.out" "mode: SIMULATION (software only)" 30
wait_for "$WORKDIR/b.out" "mode: SIMULATION (software only)" 30
wait_for "$WORKDIR/a.out" "link: connected" 30
wait_for "$WORKDIR/b.out" "link: connected" 30
# The agents now play unassisted. A finished game shows the same outcome line
# on both sides. Agent failure must be visible as a pause, never a fallback.
outcome=""
for _ in $(seq 1 "$WAIT_SECONDS"); do
  if grep -qE "X wins|O wins|draw" "$WORKDIR/a.out" && grep -qE "X wins|O wins|draw" "$WORKDIR/b.out"; then
    outcome="finished"
    break
  fi
  if grep -q "agent paused" "$WORKDIR/a.out" || grep -q "agent paused" "$WORKDIR/b.out"; then
    outcome="agent-paused"
    break
  fi
  sleep 1
done
if [[ "$outcome" != finished ]]; then
  echo "FAIL: agent game did not finish (outcome=${outcome:-timeout})" >&2
  tail -20 "$WORKDIR/a.out" "$WORKDIR/b.out" >&2 || true
  exit 1
fi
a_result=$(grep -oE "X wins|O wins|draw" "$WORKDIR/a.out" | tail -1)
b_result=$(grep -oE "X wins|O wins|draw" "$WORKDIR/b.out" | tail -1)
[[ "$a_result" == "$b_result" ]] || { echo "FAIL: outcome mismatch A=$a_result B=$b_result" >&2; exit 1; }
grep -q "agent chose" "$WORKDIR/a.out" || { echo 'FAIL: A never recorded an agent move' >&2; exit 1; }
grep -q "agent chose" "$WORKDIR/b.out" || { echo 'FAIL: B never recorded an agent move' >&2; exit 1; }
echo "PASS agent-vs-agent sim game finished: $a_result (both boards agree)"
printf '/q\n' >&3
printf '/q\n' >&4
