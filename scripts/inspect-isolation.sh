#!/usr/bin/env bash
# Host-side inspection of the two demo containers.
set -euo pipefail

fail=0
for name in ccc-a ccc-b; do
  if ! docker inspect "$name" >/dev/null 2>&1; then
    echo "missing container $name"
    fail=1
    continue
  fi
  echo "== $name =="
  docker inspect --format 'network={{.HostConfig.NetworkMode}} privileged={{.HostConfig.Privileged}} pid={{.HostConfig.PidMode}} ipc={{.HostConfig.IpcMode}} readonly={{.HostConfig.ReadonlyRootfs}}' "$name"
  socks=$(docker inspect --format '{{range .Mounts}}{{.Source}} -> {{.Destination}}{{println}}{{end}}' "$name")
  echo "mounts:"
  echo "$socks"
  if echo "$socks" | grep -q docker.sock; then
    echo "FAIL: docker.sock mounted"
    fail=1
  fi
  net=$(docker inspect --format '{{.HostConfig.NetworkMode}}' "$name")
  if [[ "$net" != "none" ]]; then
    echo "FAIL: network is $net (want none)"
    fail=1
  fi
done

if [[ "$fail" -ne 0 ]]; then
  exit 1
fi
echo "host policy looks consistent with the container plan"
echo "this does not prove the hardware channel works"
