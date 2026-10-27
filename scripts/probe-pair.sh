#!/usr/bin/env bash
set -euo pipefail
IMAGE="${IMAGE:-cpu-cache-chat:local}"
TRIALS="${TRIALS:-1}"
BITS="${BITS:-64}"
SLOT_MS="${SLOT_MS:-2}"
docker info >/dev/null
common=(--rm --init --network none --ipc private --read-only --tmpfs '/tmp:rw,nosuid,size=64m'
  --security-opt no-new-privileges --cap-drop ALL --user 65532:65532 --memory 512m)
if [[ -n "${CPUSET:-}" ]]; then common+=(--cpuset-cpus "$CPUSET"); fi
# Logs are collected only after both independent processes exit; they are never
# passed to either peer. Each run uses separate container writable storage.
logdir=$(mktemp -d "${TMPDIR:-/tmp}/ccc-probe.XXXXXX")
receiver_pid=''
sender_pid=''
cleanup() {
  [[ -z "$receiver_pid" ]] || kill "$receiver_pid" 2>/dev/null || true
  [[ -z "$sender_pid" ]] || kill "$sender_pid" 2>/dev/null || true
  docker rm -f ccc-probe-a ccc-probe-b >/dev/null 2>&1 || true
  rm -rf "$logdir"
}
trap cleanup EXIT
for name in ccc-probe-a ccc-probe-b; do
  if docker inspect "$name" >/dev/null 2>&1; then
    echo "$name already exists; remove it explicitly before probing" >&2
    trap - EXIT; rm -rf "$logdir"; exit 1
  fi
done
for channel in cache cpu; do
  echo "=== $channel probe ==="
  docker run --name ccc-probe-b "${common[@]}" "$IMAGE" probe --role b --channel "$channel" --trials "$TRIALS" --bits "$BITS" --slot-ms "$SLOT_MS" >"$logdir/b" 2>&1 &
  receiver_pid=$!
  # Give the receiver time to start and calibrate; this does not relay data.
  sleep 2
  docker run --name ccc-probe-a "${common[@]}" "$IMAGE" probe --role a --channel "$channel" --trials "$TRIALS" --bits "$BITS" --slot-ms "$SLOT_MS" >"$logdir/a" 2>&1 &
  sender_pid=$!
  status=0
  wait "$sender_pid" || status=1
  sender_pid=''
  wait "$receiver_pid" || status=1
  receiver_pid=''
  echo '--- sender A ---'; cat "$logdir/a"
  echo '--- receiver B ---'; cat "$logdir/b"
  if [[ "$status" -ne 0 ]]; then echo "FAIL: $channel probe process failed" >&2; exit 1; fi
done
