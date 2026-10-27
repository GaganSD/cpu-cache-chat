#!/usr/bin/env bash
# Launch two isolated containers. The host does not forward messages.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHANNEL="${CHANNEL:-cache}"
MODE="${MODE:-chat}"
IMAGE="${IMAGE:-cpu-cache-chat:local}"

if [[ "${CHANNEL}" != "cache" && "${CHANNEL}" != "cpu" ]]; then
  echo "CHANNEL must be cache or cpu (not sim) for the isolated demo" >&2
  exit 1
fi

if ! docker info >/dev/null 2>&1; then
  echo "docker daemon is not reachable" >&2
  exit 1
fi

common=(
  --rm
  --init
  --network none
  --ipc private
  --read-only
  --tmpfs /tmp:rw,nosuid,size=64m
  --security-opt no-new-privileges
  --cap-drop ALL
  --user 65532:65532
  --memory 512m
  --cpus 1
)

echo "Starting ${IMAGE} role A/B channel=${CHANNEL} mode=${MODE}"
echo "Attach: docker attach ccc-a    and    docker attach ccc-b"

docker rm -f ccc-a ccc-b >/dev/null 2>&1 || true

docker run -d --name ccc-a --hostname ccc-a -it "${common[@]}" \
  --cpuset-cpus 0 \
  "${IMAGE}" "${MODE}" --role a --channel "${CHANNEL}"

docker run -d --name ccc-b --hostname ccc-b -it "${common[@]}" \
  --cpuset-cpus 0 \
  "${IMAGE}" "${MODE}" --role b --channel "${CHANNEL}"

echo "containers started. Host must not relay payloads."
echo "inspect: ${ROOT}/scripts/inspect-isolation.sh"
