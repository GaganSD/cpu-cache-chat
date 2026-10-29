#!/usr/bin/env bash
# Sequential physical-campaign runner. Counts and budgets are fixed before
# collection; every attempt is retained, pass or fail. Never parallel: the
# carrier is timing-sensitive and the host has 8 GB.
#
# Chat campaigns run the qualification controls (silent no-sender window +
# receiver-blind probe) ONCE per immutable configuration (image id + channel +
# slot + cpuset + recorded load note) into a control bundle; every chat
# attempt references the bundle manifest instead of re-running controls. The
# bundle is a single qualification run, not a reliability guarantee. If the
# controls do not pass, the campaign aborts before any attempt.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${1:?usage: run-physical-matrix.sh chat|game|agent COUNT}"; COUNT="${2:?count}"
[[ "$MODE" == chat || "$MODE" == game || "$MODE" == agent ]] || { echo "bad MODE $MODE" >&2; exit 1; }
[[ "$COUNT" =~ ^[0-9]{1,2}$ ]] || { echo 'COUNT must be 1..99' >&2; exit 1; }
CHANNEL="${CHANNEL:-cpu}"
SLOT_MS="${SLOT_MS:-50}"
CPUSET="${CPUSET:-0}"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
LOAD_NOTE=$(printf '%s' "${CCC_LOAD_NOTE:-unspecified}" | tr '\n' ' ')
if [[ ! "$SLOT_MS" =~ ^[0-9]{1,3}$ ]] || (( 10#$SLOT_MS < 1 || 10#$SLOT_MS > 500 )); then
  echo 'SLOT_MS must be 1..500' >&2; exit 1
fi
SLOT_MS=$((10#$SLOT_MS))
STAMP=$(date -u +%Y-%m-%dT%H%M%SZ)
BASE="${EVIDENCE_DIR:-$ROOT/measurements/physical-matrix}/$CHANNEL-$MODE-$STAMP"
mkdir -p "$BASE"
chmod 700 "$BASE"
SUMMARY="$BASE/summary.jsonl"
echo "Matrix: mode=$MODE count=$COUNT channel=$CHANNEL slot=${SLOT_MS}ms cpuset=$CPUSET evidence=$BASE"
echo "Budgets (fixed before collection): chat wait=900s, game wait=3600s, agent wait=3600s"
CONTROLS_DIR="${CCC_CONTROLS_DIR:-$BASE/controls}"
if [[ "$MODE" == chat ]]; then
  echo "Qualification controls: bundle $CONTROLS_DIR (one run per image+channel+slot+cpuset+load-note; single qualification run, not a reliability guarantee)"
  if ! IMAGE="$IMAGE" CHANNEL="$CHANNEL" SLOT_MS="$SLOT_MS" CPUSET="$CPUSET" CCC_LOAD_NOTE="$LOAD_NOTE" \
    "$ROOT/scripts/run-controls.sh" --out "$CONTROLS_DIR" 2>&1 | tee "$BASE/controls.log"; then
    echo 'Qualification controls did not pass; campaign aborted before any chat attempt' >&2
    exit 1
  fi
fi
pass=0
first_pass=0
for ((i = 1; i <= COUNT; i++)); do
  attempt="$BASE/attempt-$i"
  mkdir -p "$attempt"
  start=$(date +%s)
  status=0
  case "$MODE" in
    chat)
      CCC_CONTROLS_DIR="$CONTROLS_DIR" CCC_LOAD_NOTE="$LOAD_NOTE" \
        IMAGE="$IMAGE" CHANNEL="$CHANNEL" SLOT_MS="$SLOT_MS" CPUSET="$CPUSET" WAIT_SECONDS=900 \
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
  # Let the VM and scheduler settle after force-removing the previous
  # attempt's CPU-burning containers; back-to-back starts correlated with
  # handshake starvation in the 25ms game campaign.
  if (( i < COUNT )); then sleep "${SETTLE_SECONDS:-15}"; fi
  rundir=$(find "$attempt" -mindepth 1 -maxdepth 1 -type d -name 'ccc-*' | head -n 1)
  retries=0
  if [[ -n "$rundir" ]]; then
    retries=$(cat "$rundir"/a.out "$rundir"/b.out 2>/dev/null | grep -c retransmit || true)
  fi
  phase=$(sed -n 's/^phase=\([a-z][a-z]*\) ts=[0-9]*$/\1/p' "$attempt/harness.log" | tail -n 1)
  first=false
  if [[ "$status" == 0 && "$retries" == 0 ]]; then first=true; fi
  phase_json=null
  if [[ -n "$phase" ]]; then phase_json="\"$phase\""; fi
  printf '{"attempt":%d,"exit":%d,"elapsed_seconds":%d,"first_attempt_success":%s,"retries":%d,"phase_reached":%s}\n' \
    "$i" "$status" "$elapsed" "$first" "$retries" "$phase_json" >> "$SUMMARY"
  if [[ "$status" == 0 ]]; then
    pass=$((pass + 1))
    if [[ "$first" == true ]]; then first_pass=$((first_pass + 1)); fi
    echo "attempt $i: PASS (${elapsed}s, phase=${phase:-none}, retries=$retries)"
  else
    echo "attempt $i: FAIL exit=$status (${elapsed}s, phase=${phase:-none}, retries=$retries; evidence retained)"
  fi
done
echo "First-attempt successes (passed with no observed retransmit): $first_pass/$COUNT"
echo "Eventual passes: $pass/$COUNT"
echo "Matrix result: $pass/$COUNT passed. Evidence: $BASE"
[[ "$pass" == "$COUNT" ]]
