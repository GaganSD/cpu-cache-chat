#!/usr/bin/env bash
# Headless two-container run on OrbStack/Docker Engine.
# Physical channel by default. Set CHANNEL=cpu to try contention.
# This is not proof of cache eviction even if messages get through.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
CHANNEL="${CHANNEL:-cpu}"
MODE="${MODE:-chat}"
WORKDIR="${TMPDIR:-/tmp}/ccc-dockere2e-$$"
mkdir -p "$WORKDIR"
A_IN="$WORKDIR/a.in"
B_IN="$WORKDIR/b.in"
A_OUT="$WORKDIR/a.out"
B_OUT="$WORKDIR/b.out"
mkfifo "$A_IN" "$B_IN"
cleanup() {
  exec 3>&- 4>&- || true
  docker rm -f ccc-e2e-a ccc-e2e-b >/dev/null 2>&1 || true
  rm -rf "$WORKDIR"
}
trap cleanup EXIT

docker info >/dev/null
docker inspect "$IMAGE" >/dev/null

common=(
  --rm -i
  --init
  --network none
  --ipc private
  --read-only
  --tmpfs /tmp:rw,nosuid,size=64m
  --security-opt no-new-privileges
  --cap-drop ALL
  --user 65532:65532
  --memory 256m
  --cpus 1
)

: >"$A_OUT"
: >"$B_OUT"
docker run "${common[@]}" --name ccc-e2e-a --hostname ccc-e2e-a \
  --cpuset-cpus 0 \
  "$IMAGE" "$MODE" --role a --channel "$CHANNEL" --plain --slot-ms "${SLOT_MS:-5}" \
  <"$A_IN" >"$A_OUT" 2>&1 &
A_PID=$!
docker run "${common[@]}" --name ccc-e2e-b --hostname ccc-e2e-b \
  --cpuset-cpus 0 \
  "$IMAGE" "$MODE" --role b --channel "$CHANNEL" --plain --slot-ms "${SLOT_MS:-5}" \
  <"$B_IN" >"$B_OUT" 2>&1 &
B_PID=$!
exec 3>"$A_IN"
exec 4>"$B_IN"

wait_for() {
  local file="$1" needle="$2" tries="${3:-480}"
  for _ in $(seq 1 "$tries"); do
    if grep -q -- "$needle" "$file"; then
      return 0
    fi
    if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then
      echo "container process exited early" >&2
      break
    fi
    sleep 0.25
  done
  echo "timeout waiting for '$needle' in $file" >&2
  echo "----- A -----" >&2
  cat "$A_OUT" >&2 || true
  echo "----- B -----" >&2
  cat "$B_OUT" >&2 || true
  return 1
}

# Isolation inspect on the live names.
sleep 1
if docker inspect ccc-e2e-a >/dev/null 2>&1; then
  docker inspect --format 'A network={{.HostConfig.NetworkMode}} privileged={{.HostConfig.Privileged}}' ccc-e2e-a
  docker inspect --format 'B network={{.HostConfig.NetworkMode}} privileged={{.HostConfig.Privileged}}' ccc-e2e-b
fi

if ! wait_for "$A_OUT" "link: connected" 240; then
  echo "FAIL: no hardware handshake (channel=$CHANNEL). Recorded as unusable, not a silent fallback."
  exit 2
fi
wait_for "$B_OUT" "link: connected" 80

printf 'hello-from-a\n' >&3
if wait_for "$B_OUT" "peer: hello-from-a" 240; then
  echo "PASS docker $CHANNEL A->B"
else
  echo "FAIL docker $CHANNEL A->B"
  exit 3
fi
printf 'hello-from-b\n' >&4
if wait_for "$A_OUT" "peer: hello-from-b" 240; then
  echo "PASS docker $CHANNEL B->A"
else
  echo "FAIL docker $CHANNEL B->A"
  exit 4
fi
printf '/q\n' >&3
printf '/q\n' >&4
wait "$A_PID" "$B_PID" || true
