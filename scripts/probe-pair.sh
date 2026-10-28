#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
TRIALS="${TRIALS:-1}"
BITS="${BITS:-64}"
SLOT_MS="${SLOT_MS:-2}"
CHANNEL="${CHANNEL:-both}"
case "$CHANNEL" in
  both) channels=(cache cpu) ;;
  cache|cpu) channels=("$CHANNEL") ;;
  *) echo "CHANNEL must be cache, cpu, or both" >&2; exit 2 ;;
esac
[[ "$TRIALS" == 1 ]] || { echo 'TRIALS must be 1 (one-shot probe)' >&2; exit 2; }
docker info >/dev/null
common=(--rm --init --network none --ipc private --cgroupns private --read-only --tmpfs '/tmp:rw,nosuid,size=64m'
  --security-opt no-new-privileges --cap-drop ALL --user 65532:65532 --memory 512m)
if [[ -n "${CPUSET:-}" ]]; then common+=(--cpuset-cpus "$CPUSET"); fi
if [[ -n "${CCC_DEBUG:-}" ]]; then common+=(-e "CCC_DEBUG=$CCC_DEBUG"); fi
# Logs are collected only after both independent processes exit; they are never
# passed to either peer. Each run uses separate container writable storage.
logdir=$(mktemp -d "${TMPDIR:-/tmp}/ccc-probe.XXXXXX")
receiver_pid=''
sender_pid=''
name_a=ccc-probe-a-$$
name_b=ccc-probe-b-$$
# Invoked by the EXIT trap.
# EXIT trap callback; ShellCheck 0.9/0.11 disagree about reachability.
# shellcheck disable=SC2317,SC2329
cleanup() {
  [[ -z "$receiver_pid" ]] || kill "$receiver_pid" 2>/dev/null || true
  [[ -z "$sender_pid" ]] || kill "$sender_pid" 2>/dev/null || true
  docker rm -f "$name_a" "$name_b" >/dev/null 2>&1 || true
  rm -rf "$logdir"
}
trap cleanup EXIT
failed=0
for channel in "${channels[@]}"; do
  echo "=== $channel probe ==="
  docker run --name "$name_b" "${common[@]}" "$IMAGE" probe --role b --channel "$channel" --trials "$TRIALS" --bits "$BITS" --slot-ms "$SLOT_MS" >"$logdir/b" 2>"$logdir/b.err" &
  receiver_pid=$!
  # Give the receiver time to start and calibrate; this does not relay data.
  sleep 2
  docker run --name "$name_a" "${common[@]}" "$IMAGE" probe --role a --channel "$channel" --trials "$TRIALS" --bits "$BITS" --slot-ms "$SLOT_MS" >"$logdir/a" 2>"$logdir/a.err" &
  sender_pid=$!
  status=0
  wait "$sender_pid" || status=1
  sender_pid=''
  wait "$receiver_pid" || status=1
  receiver_pid=''
  echo '--- sender A ---'; cat "$logdir/a"
  echo '--- receiver B ---'; cat "$logdir/b"
  # Compare only after both processes exit: neither peer receives the other's log.
  if ! python3 "$ROOT/scripts/probe-report.py" compare "$logdir/a" "$logdir/b"
  then
    echo "FAIL: $channel probe comparison" >&2
    failed=1
  fi
  if [[ "$status" -ne 0 ]]; then echo "FAIL: $channel probe process failed" >&2; failed=1; fi
done
exit "$failed"
