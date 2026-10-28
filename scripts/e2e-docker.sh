#!/usr/bin/env bash
# Headless isolated container run. No host data is passed between peers.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
CHANNEL="${CHANNEL:-cpu}"
MODE="${MODE:-chat}"
SLOT_MS="${SLOT_MS:-50}"
CPUSET="${CPUSET:-0}"
WAIT_SECONDS="${WAIT_SECONDS:-60}"
[[ "$CHANNEL" == cpu || "$CHANNEL" == cache ]] || { echo 'CHANNEL must be cpu or cache' >&2; exit 1; }
[[ "$MODE" == chat ]] || { echo 'e2e-docker currently supports MODE=chat only' >&2; exit 1; }
if [[ ! "$SLOT_MS" =~ ^[0-9]{1,3}$ ]] || (( 10#$SLOT_MS < 1 || 10#$SLOT_MS > 500 )); then
  echo 'SLOT_MS must be 1..500' >&2; exit 1
fi
SLOT_MS=$((10#$SLOT_MS))
# Three complete empty protocol control frame windows, rounded up to seconds.
SILENT_SECONDS=$(((3 * (156 + 13 * 19) * SLOT_MS + 999) / 1000))
[[ "$WAIT_SECONDS" =~ ^[0-9]{1,4}$ ]] || { echo 'WAIT_SECONDS must be an integer from 1 to 3600' >&2; exit 1; }
WAIT_SECONDS=$((10#$WAIT_SECONDS))
(( WAIT_SECONDS >= 1 && WAIT_SECONDS <= 3600 )) || { echo 'WAIT_SECONDS must be an integer from 1 to 3600' >&2; exit 1; }
WAIT_TRIES=$((WAIT_SECONDS * 4))
# Evidence is host-side only. No container receives a mount of this directory.
# Make a new private directory for each run so previous evidence is not replaced.
umask 077
if [[ -n "${EVIDENCE_DIR:-}" ]]; then
  mkdir -p -- "$EVIDENCE_DIR"
  workdir=$(mktemp -d "$EVIDENCE_DIR/ccc-dockere2e.XXXXXX")
  retain_evidence=1
  echo "Evidence directory: $workdir"
else
  workdir=$(mktemp -d "${TMPDIR:-/tmp}/ccc-dockere2e.XXXXXX")
  retain_evidence=0
fi
prefix="ccc-e2e-$$"
a="$prefix-a"; b="$prefix-b"
owned_a=0; owned_b=0
cleanup() {
  local result=$? pid
  exec 3>&- 4>&- || true
  if [[ "$owned_a" == 1 ]]; then docker rm -f "$a" >/dev/null 2>&1 || true; fi
  if [[ "$owned_b" == 1 ]]; then docker rm -f "$b" >/dev/null 2>&1 || true; fi
  # The attached Docker clients can still be draining logs after removal.
  for pid in "${probe_a:-}" "${probe_b:-}" "${A_PID:-}" "${B_PID:-}"; do
    [[ -z "$pid" ]] || wait "$pid" 2>/dev/null || true
  done
  if [[ "$result" != 0 ]]; then
    # Keep stdout and stderr separate: container debug writes can otherwise
    # splice themselves into the middle of a delivery-state line.
    local log
    for log in silent.out silent.err probe-a.out probe-a.err probe-b.out probe-b.err a.out a.err b.out b.err; do
      if [[ -f "$workdir/$log" ]]; then
        printf '%s\n' "----- $log (last 30 lines) -----" >&2
        tail -30 "$workdir/$log" >&2 || true
      fi
    done
  fi
  if [[ "$retain_evidence" == 1 ]]; then
    rm -f "$workdir/a.in" "$workdir/b.in"
    printf 'exit_status=%s\nchannel=%s\nmode=%s\nslot_ms=%s\ncpuset=%s\nwait_seconds=%s\nimage=%s\n' \
      "$result" "$CHANNEL" "$MODE" "$SLOT_MS" "$CPUSET" "$WAIT_SECONDS" "$IMAGE" > "$workdir/run.txt"
    echo "Evidence retained: $workdir"
  else
    rm -rf "$workdir"
  fi
}
trap cleanup EXIT
for name in "$a" "$b"; do
  if docker inspect "$name" >/dev/null 2>&1; then
    echo "$name already exists; refusing to remove it" >&2
    exit 1
  fi
done
docker info >/dev/null
docker image inspect "$IMAGE" >/dev/null
common=(--init --network none --ipc private --read-only
  --tmpfs '/tmp:rw,nosuid,size=64m' --security-opt no-new-privileges
  --cap-drop ALL --user 65532:65532 --memory 256m --pids-limit 128 --cpuset-cpus "$CPUSET")
if [[ "${CCC_DEBUG:-}" == 1 ]]; then common+=(--env CCC_DEBUG=1); fi

# A control with no sender must time out without decoding a frame. The report
# remains diagnostic; this alone cannot establish a working transport.
echo "Silent no-sender control: ${SILENT_SECONDS}s (three nominal empty control-frame windows); unrelated-load control not run."
docker create --name "$b" "${common[@]}" "$IMAGE" probe --role b --channel "$CHANNEL" \
  --slot-ms "$SLOT_MS" --silent --silent-seconds "$SILENT_SECONDS" >/dev/null
owned_b=1
docker start -a "$b" >"$workdir/silent.out" 2>"$workdir/silent.err" || { echo 'FAIL: silent control process failed'; exit 1; }
cat "$workdir/silent.out"
if ! grep -Fq 'false decoded frame=false' "$workdir/silent.out" || ! grep -Fq 'result=Err(Timeout)' "$workdir/silent.out"; then
  echo 'FAIL: silent sender control did not pass'; exit 1
fi
docker rm "$b" >/dev/null
owned_b=0

# Probe receives without knowing the payload. Compare logs only after both
# processes exit; no expected bytes are passed to the receiver.
docker create --name "$b" "${common[@]}" "$IMAGE" probe --role b --channel "$CHANNEL" --bits 32 --slot-ms "$SLOT_MS" >/dev/null
owned_b=1
docker create --name "$a" "${common[@]}" "$IMAGE" probe --role a --channel "$CHANNEL" --bits 32 --slot-ms "$SLOT_MS" >/dev/null
owned_a=1
"$ROOT/scripts/inspect-isolation.sh" "$a" "$b"
docker start -a "$b" >"$workdir/probe-b.out" 2>"$workdir/probe-b.err" & probe_b=$!
sleep 2
docker start -a "$a" >"$workdir/probe-a.out" 2>"$workdir/probe-a.err" & probe_a=$!
status=0
wait "$probe_a" || status=1
wait "$probe_b" || status=1
cat "$workdir/probe-a.out" "$workdir/probe-b.out"
[[ "$status" == 0 ]] || { echo 'FAIL: probe process failed'; exit 1; }
sent=$(sed -n 's/^sender payload .*: \[\(.*\)\]$/\1/p' "$workdir/probe-a.out" | tr -d ', ')
received=$(sed -n 's/.*received bytes (hex): \([0-9a-f]*\).*/\1/p' "$workdir/probe-b.out")
if [[ -z "$sent" || "$sent" != "$received" ]]; then
  echo 'FAIL: receiver-blind probe did not decode all expected bits; BER unavailable if no frame decoded'; exit 1
fi
echo 'PASS: probe decoded expected bytes (0 bit errors in this frame) and silent control passed'
docker rm "$a" "$b" >/dev/null
owned_a=0; owned_b=0

mkfifo "$workdir/a.in" "$workdir/b.in"
# Open read/write so neither FIFO open can hang if Docker fails early.
exec 3<>"$workdir/a.in" 4<>"$workdir/b.in"
docker create -i --name "$a" "${common[@]}" "$IMAGE" chat --role a --channel "$CHANNEL" --plain --slot-ms "$SLOT_MS" >/dev/null
owned_a=1
docker create -i --name "$b" "${common[@]}" "$IMAGE" chat --role b --channel "$CHANNEL" --plain --slot-ms "$SLOT_MS" >/dev/null
owned_b=1
"$ROOT/scripts/inspect-isolation.sh" "$a" "$b"
docker start -a -i "$a" <"$workdir/a.in" >"$workdir/a.out" 2>"$workdir/a.err" & A_PID=$!
docker start -a -i "$b" <"$workdir/b.in" >"$workdir/b.out" 2>"$workdir/b.err" & B_PID=$!
wait_for() {
  local file="$1" needle="$2" tries="${3:-$WAIT_TRIES}"
  for _ in $(seq 1 "$tries"); do
    if grep -Fq -- "$needle" "$file"; then return 0; fi
    if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
    sleep 0.25
  done
  echo "FAIL: waiting for '$needle'" >&2
  return 1
}
wait_for "$workdir/a.out" 'link: connected'
wait_for "$workdir/b.out" 'link: connected'
wait_for_delivery_state() {
  local file="$1" seq="$2" expected="$3"
  for _ in $(seq 1 "$WAIT_TRIES"); do
    if grep -Fxq "seq $seq: delivery unknown" "$file"; then
      echo "FAIL: seq $seq reached delivery unknown while waiting for $expected" >&2
      return 1
    fi
    if grep -Fxq "seq $seq: $expected" "$file"; then return 0; fi
    if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
    sleep 0.25
  done
  echo "FAIL: seq $seq did not reach $expected" >&2
  return 1
}
check_delivery() {
  local sender="$1" receiver="$2" message="$3" seq
  wait_for "$sender" "queued: $message"
  wait_for "$sender" ': transmitting'
  seq=$(sed -n 's/^seq \([0-9][0-9]*\): transmitting$/\1/p' "$sender" | head -n 1)
  [[ -n "$seq" ]] || { echo "FAIL: missing transmission sequence for $message" >&2; return 1; }
  wait_for_delivery_state "$sender" "$seq" 'awaiting ack'
  wait_for_delivery_state "$sender" "$seq" 'delivered'
  wait_for "$receiver" "peer: $message"
  # All states must belong to the same message and appear in order. An earlier
  # unknown result cannot be counted as delivery, even if a stale line exists.
  if ! awk -v message="$message" -v seq="$seq" '
    $0 == "queued: " message && !queued { queued = NR }
    $0 == "seq " seq ": transmitting" && queued && !transmitting { transmitting = NR }
    $0 == "seq " seq ": awaiting ack" && transmitting && !awaiting { awaiting = NR }
    $0 == "seq " seq ": delivery unknown" && queued && !delivered { unknown = 1 }
    $0 == "seq " seq ": delivered" && awaiting && !delivered { delivered = NR }
    END { exit !(queued && transmitting > queued && awaiting > transmitting && delivered > awaiting && !unknown) }
  ' "$sender"; then
    echo "FAIL: delivery states missing, out of order, or unknown for $message" >&2
    return 1
  fi
  echo "PASS: $message queued, transmitted, acknowledged, and received (seq $seq)"
}
printf 'hello-from-a\n' >&3
check_delivery "$workdir/a.out" "$workdir/b.out" 'hello-from-a'
printf 'hello-from-b\n' >&4
check_delivery "$workdir/b.out" "$workdir/a.out" 'hello-from-b'
echo "PASS docker $CHANNEL chat both directions"
printf '/q\n' >&3; printf '/q\n' >&4
wait "$A_PID" "$B_PID"
