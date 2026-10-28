#!/usr/bin/env bash
# Build the agent image: cpu-cache-chat binary plus Node and the Pi player.
# The base image is built first so the binary matches the checked-out source.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BASE_IMAGE="${BASE_IMAGE:-cpu-cache-chat:local}"
IMAGE="$BASE_IMAGE" "$ROOT/docker/build.sh"
REV=$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$BASE_IMAGE")
BASE_ID=$(docker image inspect --format '{{.Id}}' "$BASE_IMAGE")
docker build --build-arg CCC_IMAGE="$BASE_IMAGE" --build-arg VCS_REF="$REV" \
  -f "$ROOT/docker/Dockerfile.agent" -t "${IMAGE:-cpu-cache-chat:agent}" "$ROOT"
[[ "$BASE_ID" == "$(docker image inspect --format '{{.Id}}' "$BASE_IMAGE")" ]] || {
  echo 'Base image changed during agent build; discard this artifact and rebuild' >&2
  exit 1
}
