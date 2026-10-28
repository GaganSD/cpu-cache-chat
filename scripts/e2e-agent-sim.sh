#!/usr/bin/env bash
# Genuine Pi agents, SOFTWARE SIMULATION only. Hosted inference requires opt-in.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${BIN:-$ROOT/target/release/cpu-cache-chat}"
VERIFY="${VERIFY:-$(dirname "$BIN")/ccc-verify-game}"
if [[ ! -x "$BIN" || ! -x "$VERIFY" ]]; then
  cargo build --locked --manifest-path "$ROOT/Cargo.toml" --release -p cpu-cache-chat -p ccc-game
fi
export CCC_AGENT_PROVIDER="${CCC_AGENT_PROVIDER:-opencode-go}"
export CCC_AGENT_PATH="$ROOT/agent/src/play.mjs"
[[ "${CCC_HOSTED_OPT_IN:-}" == 1 ]] || { echo 'Set CCC_HOSTED_OPT_IN=1 for external inference and board disclosure' >&2; exit 1; }
case "$CCC_AGENT_PROVIDER" in
  opencode-go)
    [[ -n "${OPENCODE_GO_API_KEY:-}" ]] || { echo 'OPENCODE_GO_API_KEY required' >&2; exit 1; }
    export CCC_AGENT_MODEL=deepseek-v4.1-flash ;;
  vercel-ai-gateway)
    export AI_GATEWAY_API_KEY="${AI_GATEWAY_API_KEY:-${VERCEL_AI_GATEWAY_API_KEY:-}}"
    [[ -n "$AI_GATEWAY_API_KEY" ]] || { echo 'AI_GATEWAY_API_KEY required' >&2; exit 1; }
    export CCC_AGENT_MODEL=reflection/beam-free ;;
  *) echo 'This harness supports the two explicitly approved hosted providers only' >&2; exit 1 ;;
esac
WAIT_SECONDS="${WAIT_SECONDS:-600}"
[[ "$WAIT_SECONDS" =~ ^[0-9]{1,4}$ ]] || { echo 'WAIT_SECONDS must be 1..3600' >&2; exit 1; }
(( WAIT_SECONDS >= 1 && WAIT_SECONDS <= 3600 )) || exit 1
WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/ccc-agent-sim.XXXXXX")
A_PID=''; B_PID=''
cleanup() {
  local status=$?
  trap - EXIT
  for pid in "$A_PID" "$B_PID"; do
    [[ -z "$pid" ]] || kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  if [[ "$status" == 0 && -z "${RETAIN:-}" ]]; then
    rm -rf "$WORKDIR"
  else
    echo "Evidence retained (review before sharing): $WORKDIR" >&2
  fi
  exit "$status"
}
trap cleanup EXIT
mkfifo "$WORKDIR/a.in" "$WORKDIR/b.in"
"$BIN" game --role a --channel cpu --sim --sim-path "$WORKDIR/link.sim" --plain \
  --agent --events-path "$WORKDIR/a.jsonl" <"$WORKDIR/a.in" >"$WORKDIR/a.out" 2>"$WORKDIR/a.err" &
A_PID=$!
"$BIN" game --role b --channel cpu --sim --sim-path "$WORKDIR/link.sim" --plain \
  --agent --events-path "$WORKDIR/b.jsonl" <"$WORKDIR/b.in" >"$WORKDIR/b.out" 2>"$WORKDIR/b.err" &
B_PID=$!
exec 3>"$WORKDIR/a.in" 4>"$WORKDIR/b.in"
deadline=$((SECONDS + WAIT_SECONDS))
while :; do
  if grep -Fq '"type":"final"' "$WORKDIR/a.jsonl" 2>/dev/null && \
     grep -Fq '"type":"final"' "$WORKDIR/b.jsonl" 2>/dev/null; then break; fi
  if (( SECONDS >= deadline )); then echo 'FAIL: whole-run agent deadline exceeded' >&2; exit 1; fi
  if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then
    echo 'FAIL: an agent peer exited before a complete game' >&2; exit 1
  fi
  sleep 0.1
done
printf '/q\n' >&3
printf '/q\n' >&4
exec 3>&- 4>&-
wait "$A_PID"
wait "$B_PID"
A_PID=''; B_PID=''
"$VERIFY" --require-agents "$WORKDIR/a.jsonl" "$WORKDIR/b.jsonl"
echo 'PASS genuine-agent simulation: legal equal histories and successful peer exits; NOT a physical result'
