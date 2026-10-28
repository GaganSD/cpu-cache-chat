#!/usr/bin/env bash
# Sequential physical-campaign runner. Counts and budgets are fixed before
# collection; every attempt is retained, pass or fail. Never parallel: the
# carrier is timing-sensitive and the host has 8 GB.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:?usage: run-physical-matrix.sh chat|game|agent COUNT}"; COUNT="${2:?count}"
[[ "$MODE" == chat || "$MODE" == game || "$MODE" == agent ]] || { echo "bad MODE $MODE" >&2; exit 1; }
[[ "$COUNT" =~ ^[0-9]{1,2}$ ]] || { echo 'COUNT must be 1..99' >&2; exit 1; }
CHANNEL="${CHANNEL:-cpu}"
SLOT_MS="${SLOT_MS:-50}"
CPUSET="${CPUSET:-0}"
STAMP=$(date -u +%Y-%m-%dT%H%M%SZ)
BASE="${EVIDENCE_DIR:-$ROOT/measurements/physical-matrix}/$CHANNEL-$MODE-$STAMP"
mkdir -p "$BASE"
chmod 700 "$BASE"
SUMMARY="$BASE/summary.jsonl"
echo "Matrix: mode=$MODE count=$COUNT channel=$CHANNEL slot=${SLOT_MS}ms cpuset=$CPUSET evidence=$BASE"
echo "Budgets (fixed before collection): chat wait=900s, game wait=3600s, agent wait=3600s"
pass=0
for ((i = 1; i <= COUNT; i++)); do
  attempt="$BASE/attempt-$i"
  mkdir -p "$attempt"
  start=$(date +%s)
  status=0
  case "$MODE" in
    chat)
      CHANNEL="$CHANNEL" SLOT_MS="$SLOT_MS" CPUSET="$CPUSET" WAIT_SECONDS=900 \
        EVIDENCE_DIR="$attempt" "$ROOT/scripts/e2e-docker.sh" >"$attempt/harness.log" 2>&1 || status=$?
      ;;
    game)
      CHANNEL="$CHANNEL" SLOT_MS="$SLOT_MS" CPUSET="$CPUSET" WAIT_SECONDS=3600 WAIT_TRIES=14400 \
        EVIDENCE_DIR="$attempt" "$ROOT/scripts/e2e-docker-game.sh" >"$attempt/harness.log" 2>&1 || status=$?
      ;;
    agent)
      CCC_HOSTED_OPT_IN=1 CHANNEL="$CHANNEL" SLOT_MS="$SLOT_MS" CPUSET="$CPUSET" WAIT_SECONDS=3600 \
        EVIDENCE_DIR="$attempt" "$ROOT/scripts/e2e-docker-agent.sh" >"$attempt/harness.log" 2>&1 || status=$?
      ;;
  esac
  elapsed=$(( $(date +%s) - start ))
  printf '{"attempt":%d,"exit":%d,"elapsed_seconds":%d}\n' "$i" "$status" "$elapsed" >> "$SUMMARY"
  if [[ "$status" == 0 ]]; then pass=$((pass + 1)); echo "attempt $i: PASS (${elapsed}s)"; else echo "attempt $i: FAIL exit=$status (${elapsed}s; evidence retained)"; fi
done
echo "Matrix result: $pass/$COUNT passed. Evidence: $BASE"
[[ "$pass" == "$COUNT" ]]
