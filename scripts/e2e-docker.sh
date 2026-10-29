#!/usr/bin/env bash
# Headless isolated container run. No host data is passed between peers.
#
# Qualification controls (silent no-sender window + receiver-blind probe) run
# once per immutable configuration via scripts/run-controls.sh. With
# --controls-dir/CCC_CONTROLS_DIR a matching control bundle is referenced
# instead of re-running controls; without a valid bundle the controls run
# inline into this attempt's evidence (standalone behavior). A bundle is a
# single qualification run, not a reliability guarantee.
#
# One monotonic attempt deadline (WAIT_SECONDS, default 300s) covers the
# connect, chat, and verify phases; the connect phase has a subordinate
# deadline (CONNECT_SECONDS). Timestamped phase events are emitted as
# 'phase=start|silent|probe|connect|chat|verify ts=<epoch>' lines on stdout
# and in phases.log. Deadline expiration is a FAIL with the phase recorded.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
CHANNEL="${CHANNEL:-cpu}"
MODE="${MODE:-chat}"
SLOT_MS="${SLOT_MS:-50}"
CPUSET="${CPUSET:-0}"
WAIT_SECONDS="${WAIT_SECONDS:-300}"
CONNECT_SECONDS="${CONNECT_SECONDS:-120}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --controls-dir)
      [[ $# -ge 2 ]] || { echo '--controls-dir needs a directory' >&2; exit 1; }
      CCC_CONTROLS_DIR="$2"; shift 2 ;;
    *) echo 'usage: e2e-docker.sh [--controls-dir DIR]' >&2; exit 1 ;;
  esac
done
CCC_CONTROLS_DIR="${CCC_CONTROLS_DIR:-}"
LOAD_NOTE=$(printf '%s' "${CCC_LOAD_NOTE:-unspecified}" | tr '\n' ' ')
[[ "$CHANNEL" == cpu || "$CHANNEL" == cache ]] || { echo 'CHANNEL must be cpu or cache' >&2; exit 1; }
[[ "$MODE" == chat ]] || { echo 'e2e-docker currently supports MODE=chat only' >&2; exit 1; }
if [[ ! "$SLOT_MS" =~ ^[0-9]{1,3}$ ]] || (( 10#$SLOT_MS < 1 || 10#$SLOT_MS > 500 )); then
  echo 'SLOT_MS must be 1..500' >&2; exit 1
fi
SLOT_MS=$((10#$SLOT_MS))
[[ "$WAIT_SECONDS" =~ ^[0-9]{1,4}$ ]] || { echo 'WAIT_SECONDS must be an integer from 1 to 3600' >&2; exit 1; }
WAIT_SECONDS=$((10#$WAIT_SECONDS))
(( WAIT_SECONDS >= 1 && WAIT_SECONDS <= 3600 )) || { echo 'WAIT_SECONDS must be an integer from 1 to 3600' >&2; exit 1; }
[[ "$CONNECT_SECONDS" =~ ^[0-9]{1,4}$ && "$CONNECT_SECONDS" -ge 1 ]] || { echo 'CONNECT_SECONDS must be a positive integer' >&2; exit 1; }
CONNECT_SECONDS=$((10#$CONNECT_SECONDS))
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
phases_log="$workdir/phases.log"
: > "$phases_log"
CURRENT_PHASE=start
attempt_deadline=''
controls_mode=inline
controls_bundle=''
log_phase() {
  CURRENT_PHASE="$1"
  local ts
  ts=$(date +%s)
  echo "phase=$1 ts=$ts"
  printf 'phase=%s ts=%s\n' "$1" "$ts" >> "$phases_log"
}
prefix="ccc-e2e-$$"
a="$prefix-a"; b="$prefix-b"
owned_a=0; owned_b=0
cleanup() {
  local result=$? pid
  exec 3>&- 4>&- || true
  if [[ "$owned_a" == 1 ]]; then docker rm -f "$a" >/dev/null 2>&1 || true; fi
  if [[ "$owned_b" == 1 ]]; then docker rm -f "$b" >/dev/null 2>&1 || true; fi
  # The attached Docker clients can still be draining logs after removal.
  for pid in "${A_PID:-}" "${B_PID:-}"; do
    [[ -z "$pid" ]] || wait "$pid" 2>/dev/null || true
  done
  if [[ "$result" != 0 ]]; then
    # Keep stdout and stderr separate: container debug writes can otherwise
    # splice themselves into the middle of a delivery-state line.
    local log
    for log in controls/silent.out controls/silent.err controls/probe-a.out controls/probe-a.err \
      controls/probe-b.out controls/probe-b.err a.out a.err b.out b.err phases.log; do
      if [[ -f "$workdir/$log" ]]; then
        printf '%s\n' "----- $log (last 30 lines) -----" >&2
        tail -30 "$workdir/$log" >&2 || true
      fi
    done
  fi
  if [[ "$retain_evidence" == 1 ]]; then
    rm -f "$workdir/a.in" "$workdir/b.in"
    {
      printf 'exit_status=%s\nchannel=%s\nmode=%s\nslot_ms=%s\ncpuset=%s\nwait_seconds=%s\nimage=%s\n' \
        "$result" "$CHANNEL" "$MODE" "$SLOT_MS" "$CPUSET" "$WAIT_SECONDS" "$IMAGE"
      printf 'image_id=%s\n' "${image_id:-unknown}"
      printf 'controls=%s\n' "$controls_mode"
      printf 'controls_bundle=%s\n' "${controls_bundle:--}"
      printf 'phase_reached=%s\n' "$(sed -n 's/^phase=\([a-z][a-z]*\) ts=[0-9]*$/\1/p' "$phases_log" | tail -n 1)"
      printf 'attempt_deadline=%s\n' "${attempt_deadline:--}"
    } > "$workdir/run.txt"
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
image_id=$(docker image inspect "$IMAGE" --format '{{.Id}}')
[[ -n "$image_id" ]] || { echo "FAIL: cannot resolve image id for $IMAGE" >&2; exit 1; }
common=(--init --network none --ipc private --cgroupns private --read-only
  --tmpfs '/tmp:rw,nosuid,size=64m' --security-opt no-new-privileges
  --cap-drop ALL --user 65532:65532 --memory 256m --pids-limit 128 --cpuset-cpus "$CPUSET")
if [[ "${CCC_DEBUG:-}" == 1 ]]; then common+=(--env CCC_DEBUG=1); fi
log_phase start

# Keep this canonical string in sync with scripts/run-controls.sh.
sha256_of() { printf '%s' "$1" | { sha256sum 2>/dev/null || shasum -a 256; } | awk '{print $1}'; }
config_sha=$(sha256_of "ccc-controls-v1|$image_id|$CHANNEL|$SLOT_MS|$CPUSET|$LOAD_NOTE")
controls_bundle_valid() {
  local dir="$1" manifest="$1/manifest.txt"
  [[ -f "$manifest" && -f "$dir/silent.out" && -f "$dir/probe-a.out" && -f "$dir/probe-b.out" ]] || return 1
  [[ $(sed -n 's/^config_sha256=//p' "$manifest" 2>/dev/null) == "$config_sha" ]] || return 1
  python3 "$ROOT/scripts/probe-report.py" bundle "$dir" >/dev/null
}
if [[ -n "$CCC_CONTROLS_DIR" ]]; then
  if controls_bundle_valid "$CCC_CONTROLS_DIR"; then
    controls_mode=bundle
    controls_bundle="$CCC_CONTROLS_DIR"
    cp "$CCC_CONTROLS_DIR/manifest.txt" "$workdir/controls-manifest.txt"
    echo "Qualification controls: referencing bundle $CCC_CONTROLS_DIR (config ${config_sha:0:12}...); single qualification run, not a reliability guarantee"
  else
    echo "Qualification controls: no valid bundle for this configuration at $CCC_CONTROLS_DIR; running controls inline"
  fi
fi
if [[ "$controls_mode" == inline ]]; then
  CURRENT_PHASE=silent
  IMAGE="$IMAGE" CHANNEL="$CHANNEL" SLOT_MS="$SLOT_MS" CPUSET="$CPUSET" \
    CCC_LOAD_NOTE="$LOAD_NOTE" CCC_DEBUG="${CCC_DEBUG:-}" CCC_PHASES_LOG="$phases_log" \
    "$ROOT/scripts/run-controls.sh" --out "$workdir/controls" || { echo 'FAIL: qualification controls did not pass'; exit 1; }
  controls_bundle="$workdir/controls"
fi

# One monotonic deadline for the whole chat attempt; the connect phase gets a
# subordinate deadline so a stalled handshake cannot consume the chat budget.
log_phase connect
attempt_deadline=$(( $(date +%s) + WAIT_SECONDS ))
connect_deadline=$(( $(date +%s) + CONNECT_SECONDS ))
if (( connect_deadline > attempt_deadline )); then connect_deadline=$attempt_deadline; fi
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
  local file="$1" needle="$2" deadline="${3:-$attempt_deadline}"
  while :; do
    if grep -Fq -- "$needle" "$file"; then return 0; fi
    if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
    if (( $(date +%s) >= deadline )); then
      echo "FAIL: attempt deadline expired during phase $CURRENT_PHASE waiting for '$needle'" >&2
      return 1
    fi
    sleep 0.25
  done
  echo "FAIL: waiting for '$needle'" >&2
  return 1
}
wait_for "$workdir/a.out" 'link: connected' "$connect_deadline"
wait_for "$workdir/b.out" 'link: connected' "$connect_deadline"
wait_for_delivery_state() {
  local file="$1" seq="$2" expected="$3"
  while :; do
    if grep -Fxq "seq $seq: delivery unknown" "$file"; then
      echo "FAIL: seq $seq reached delivery unknown while waiting for $expected" >&2
      return 1
    fi
    if grep -Fxq "seq $seq: $expected" "$file"; then return 0; fi
    if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
    if (( $(date +%s) >= attempt_deadline )); then
      echo "FAIL: attempt deadline expired during phase $CURRENT_PHASE waiting for seq $seq $expected" >&2
      return 1
    fi
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
# Prequeue each peer's independent message as soon as both sides report
# 'link: connected'. Host-side input readiness is not protocol relay; each
# client drains its own FIFO when its protocol loop is ready.
log_phase chat
printf 'hello-from-a\n' >&3
printf 'hello-from-b\n' >&4
check_delivery "$workdir/a.out" "$workdir/b.out" 'hello-from-a'
check_delivery "$workdir/b.out" "$workdir/a.out" 'hello-from-b'
log_phase verify
echo "PASS docker $CHANNEL chat both directions"
printf '/q\n' >&3; printf '/q\n' >&4
# Bounded drain: clients exit on /q. A client that ignores /q is removed by
# the cleanup trap instead of blocking past the attempt deadline.
drain_deadline=$(( $(date +%s) + 15 ))
while kill -0 "$A_PID" 2>/dev/null || kill -0 "$B_PID" 2>/dev/null; do
  if (( $(date +%s) >= drain_deadline )); then break; fi
  sleep 0.25
done
for pid in "$A_PID" "$B_PID"; do
  if ! kill -0 "$pid" 2>/dev/null; then wait "$pid" || true; fi
done
