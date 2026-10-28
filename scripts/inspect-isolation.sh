#!/usr/bin/env bash
# Inspect configuration only; this is not a live network isolation test.
set -euo pipefail
policy=none
if [[ ${1:-} == --hosted ]]; then policy=hosted; shift; fi
if [[ $# != 2 || $1 == "$2" ]]; then
  echo 'Usage: inspect-isolation.sh [--hosted] container-a container-b' >&2; exit 2
fi
command -v python3 >/dev/null || { echo 'python3 required' >&2; exit 1; }
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
for i in 1 2; do
  name=$1; shift
  docker inspect "$name" > "$tmp/$i.json" || { echo "FAIL: cannot inspect $name" >&2; exit 1; }
done
python3 - "$policy" "$tmp/1.json" "$tmp/2.json" <<'PY'
import json, sys
policy = sys.argv[1]
try:
    cs = [json.load(open(p, encoding='utf-8')) for p in sys.argv[2:]]
    if any(not isinstance(c, list) or len(c) != 1 for c in cs):
        raise ValueError('expected one inspect result per container')
    cs = [c[0] for c in cs]
    networks = []
    for c in cs:
        h, config, n = c['HostConfig'], c['Config'], c['NetworkSettings']['Networks']
        name = c['Name']
        def require(ok, reason):
            if not ok: raise ValueError(f'{name}: {reason}')
        require(h['Privileged'] is False, 'privileged')
        require(h['PidMode'] in ('', 'private'), 'PID namespace not private')
        require(h['IpcMode'] == 'private', 'IPC namespace not private')
        require(h['UTSMode'] in ('', 'private'), 'UTS namespace not private')
        require(h['UsernsMode'] != 'host', 'host user namespace')
        require(h['CgroupnsMode'] == 'private', 'cgroup namespace not explicitly private')
        require(h['ReadonlyRootfs'] is True, 'root filesystem writable')
        require(config['User'] == '65532:65532', 'unexpected user')
        require('ALL' in (h['CapDrop'] or []), 'capabilities not dropped')
        require(not h['CapAdd'], 'added capabilities')
        opts = h['SecurityOpt'] or []
        require(any(x in ('no-new-privileges', 'no-new-privileges:true') for x in opts), 'no-new-privileges missing')
        require(all(x in ('no-new-privileges', 'no-new-privileges:true') for x in opts), 'unexpected security option')
        require(not h['Devices'] and not h.get('DeviceRequests') and not h.get('DeviceCgroupRules'), 'devices exposed')
        require(not h['PortBindings'] and not config.get('ExposedPorts') and not h['PublishAllPorts'], 'ports exposed')
        require(not c['Mounts'] and not h.get('Binds') and not h.get('VolumesFrom'), 'host or volume mounts are not allowed')
        require(h['Memory'] > 0 and h['PidsLimit'] is not None and h['PidsLimit'] > 0, 'memory/pid limits missing')
        require(bool(h['CpusetCpus']) or h.get('NanoCpus', 0) > 0, 'CPU bound missing')
        require(isinstance(n, dict), 'invalid network membership')
        if policy == 'none':
            require(h['NetworkMode'] == 'none' and set(n).issubset({'none'}), 'network not none')
        else:
            require(h['NetworkMode'] not in ('none', 'host', 'bridge', 'default', '') and len(n) == 1 and h['NetworkMode'] in n, 'expected one dedicated network')
        networks.append(set(n))
    if policy == 'hosted' and networks[0] & networks[1]:
        raise ValueError('shared network')
except (KeyError, TypeError, ValueError, OSError, json.JSONDecodeError) as e:
    print(f'FAIL: inspection: {e}', file=sys.stderr)
    sys.exit(1)
print('Configuration checks passed; live network policy is UNVERIFIED; hardware channel is not established.')
PY
