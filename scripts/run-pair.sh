#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHANNEL="${CHANNEL:-cache}"
MODE="${MODE:-chat}"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
[[ "$CHANNEL" == cache || "$CHANNEL" == cpu ]] || { echo 'CHANNEL must be cache or cpu' >&2; exit 1; }
[[ "$MODE" == chat || "$MODE" == game ]] || { echo 'MODE must be chat or game' >&2; exit 1; }
docker info >/dev/null
common=(--init --network none --ipc private --read-only --tmpfs '/tmp:rw,nosuid,size=64m'
  --security-opt no-new-privileges --cap-drop ALL --user 65532:65532 --memory 512m)
if [[ -n "${CPUSET:-}" ]]; then common+=(--cpuset-cpus "$CPUSET"); fi
# Refuse existing names instead of removing unrelated or active containers.
for name in ccc-a ccc-b; do
  if docker inspect "$name" >/dev/null 2>&1; then echo "$name already exists; remove it explicitly before starting" >&2; exit 1; fi
done
extra=()
if [[ "${HEADLESS:-}" == 1 ]]; then extra+=(--plain); fi
docker run -d -it --name ccc-a --hostname ccc-a "${common[@]}" "$IMAGE" "$MODE" --role a --channel "$CHANNEL" "${extra[@]}"
if ! docker run -d -it --name ccc-b --hostname ccc-b "${common[@]}" "$IMAGE" "$MODE" --role b --channel "$CHANNEL" "${extra[@]}"; then
  docker rm -f ccc-a >/dev/null || true
  exit 1
fi
"$ROOT/scripts/inspect-isolation.sh"
printf '%s\n' 'Attach in separate terminals: docker attach ccc-a   and   docker attach ccc-b' \
  'Detach without stopping: Ctrl-p Ctrl-q. Stop and remove: docker rm -f ccc-a ccc-b.' \
  'The host does not forward messages.'
