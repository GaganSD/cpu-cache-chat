#!/usr/bin/env bash
# Integration tests using stopped containers; no process or network is started.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
prefix="ccc-isolation-test-$(python3 -c 'import secrets; print(secrets.token_hex(8))')"
a="$prefix-a"; b="$prefix-b"
tmp=$(mktemp -d)
remove_pair() {
  docker ps -aq --filter "label=org.cpu-cache-chat.policy-test=$prefix" | while IFS= read -r id; do
    if [[ -n "$id" ]]; then docker rm -f "$id" >/dev/null; fi
  done
}
cleanup() { remove_pair >/dev/null 2>&1 || true; rm -rf "$tmp"; }
trap cleanup EXIT
common=(--label "org.cpu-cache-chat.policy-test=$prefix" --ipc private --cgroupns private --read-only --user 65532:65532 --cap-drop ALL
  --security-opt no-new-privileges --memory 128m --pids-limit 64 --cpuset-cpus "${CPUSET:-0}" --entrypoint /bin/true)
make_pair() {
  remove_pair
  docker create --name "$a" --network none "${common[@]}" "$@" "$IMAGE" >/dev/null
  docker create --name "$b" --network none "${common[@]}" "$IMAGE" >/dev/null
}
expect_fail() {
  local label=$1
  if "$ROOT/scripts/inspect-isolation.sh" "$a" "$b" >"$tmp/out" 2>&1; then
    echo "FAIL: accepted $label" >&2; cat "$tmp/out" >&2; exit 1
  fi
  echo "PASS: rejected $label"
}
make_pair
"$ROOT/scripts/inspect-isolation.sh" "$a" "$b"
remove_pair
docker create --name "$a" "${common[@]}" --network bridge "$IMAGE" >/dev/null
docker create --name "$b" "${common[@]}" --network none "$IMAGE" >/dev/null
expect_fail 'network bridge'
make_pair --privileged
expect_fail 'privileged container'
make_pair --mount type=bind,src=/var/run,dst=/host-run,readonly
expect_fail '/var/run bind'
make_pair --mount type=bind,src=/run,dst=/host-run,readonly
expect_fail '/run bind'
touch "$tmp/docker.sock"
make_pair --mount type=bind,src="$tmp/docker.sock",dst=/socket,readonly
expect_fail 'docker socket bind'
make_pair --mount type=bind,src="$tmp",dst=/shared
# Replace B with a nested read-only bind: one writable side is unsafe.
docker rm -f "$b" >/dev/null
mkdir "$tmp/nested"
docker create --name "$b" "${common[@]}" --network none --mount type=bind,src="$tmp/nested",dst=/shared,readonly "$IMAGE" >/dev/null
expect_fail 'overlapping writable bind mounts'
