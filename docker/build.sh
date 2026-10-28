#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
REV=$(git -C "$ROOT" rev-parse --verify HEAD 2>/dev/null || printf unknown)
if [[ -n "$(git -C "$ROOT" status --porcelain 2>/dev/null)" ]]; then REV="$REV-dirty"; fi
docker build --build-arg VCS_REF="$REV" -f "$ROOT/docker/Dockerfile" -t "$IMAGE" "$ROOT"
