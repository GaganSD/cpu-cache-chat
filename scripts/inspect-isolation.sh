#!/usr/bin/env bash
set -euo pipefail
fail=0
mounts_a=''
mounts_b=''
for name in ccc-a ccc-b; do
  if ! details=$(docker inspect --format '{{.HostConfig.NetworkMode}}|{{.HostConfig.Privileged}}|{{.HostConfig.PidMode}}|{{.HostConfig.IpcMode}}' "$name"); then
    echo "FAIL: cannot inspect $name" >&2; fail=1; continue
  fi
  IFS='|' read -r net privileged pid ipc <<< "$details"
  echo "$name: network=$net privileged=$privileged pid=$pid ipc=$ipc"
  [[ "$net" == none ]] || { echo "FAIL: $name network is not none" >&2; fail=1; }
  [[ "$privileged" == false ]] || { echo "FAIL: $name is privileged" >&2; fail=1; }
  [[ -z "$pid" ]] || { echo "FAIL: $name does not use a private PID namespace" >&2; fail=1; }
  [[ "$ipc" == private ]] || { echo "FAIL: $name IPC is not private" >&2; fail=1; }
  mounts=$(docker inspect --format '{{range .Mounts}}{{.Type}}|{{.Source}}|{{.Destination}}|{{.RW}}{{println}}{{end}}' "$name")
  printf '%s mounts:\n%s\n' "$name" "$mounts"
  while IFS='|' read -r type source dest _; do
    [[ -n "$type" ]] || continue
    if [[ "$source" == *docker.sock* || "$dest" == *docker.sock* \
       || "$source" == /var/run || "$source" == /run \
       || "$source" == /var/run/* || "$source" == /run/* ]]; then
      echo "FAIL: $name exposes a docker host socket path ($source -> $dest)" >&2; fail=1
    fi
  done <<< "$mounts"
  if [[ "$name" == ccc-a ]]; then mounts_a="$mounts"; else mounts_b="$mounts"; fi
done
# Bind paths can overlap without being identical. Docker supplies resolved source paths.
# A read-only mount on one side is still unsafe if the other side can write it.
while IFS='|' read -r type_a source_a _ rw_a; do
  [[ "$type_a" == bind || "$type_a" == volume ]] || continue
  while IFS='|' read -r type_b source_b _ rw_b; do
    [[ "$type_b" == bind || "$type_b" == volume ]] || continue
    [[ -n "$source_a" && -n "$source_b" ]] || continue
    source_a="${source_a%/}"; source_b="${source_b%/}"
    [[ -n "$source_a" ]] || source_a=/
    [[ -n "$source_b" ]] || source_b=/
    if [[ "$source_a" == "$source_b" || "$source_a" == / || "$source_b" == / || "$source_a" == "$source_b/"* || "$source_b" == "$source_a/"* ]]; then
      if [[ "$rw_a" == true || "$rw_b" == true ]]; then
        echo "FAIL: shared writable mount sources $source_a and $source_b" >&2; fail=1
      fi
    fi
  done <<< "$mounts_b"
done <<< "$mounts_a"
[[ "$fail" -eq 0 ]] || exit 1
echo 'Container configuration checks passed; this does not establish a working hardware channel.'
