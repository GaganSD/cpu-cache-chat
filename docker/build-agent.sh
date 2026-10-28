#!/usr/bin/env bash
# Build the agent image: cpu-cache-chat binary plus Node and the Pi player.
# The base image is built first so the binary matches the checked-out source.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/docker/build.sh"
docker build -f "$ROOT/docker/Dockerfile.agent" -t "${IMAGE:-cpu-cache-chat:agent}" "$ROOT"
