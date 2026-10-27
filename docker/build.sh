#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
docker build -f "$ROOT/docker/Dockerfile" -t "$IMAGE" "$ROOT"
