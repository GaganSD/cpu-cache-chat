#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/bin"
python3 - "$tmp" <<'PY'
import json, sys
from pathlib import Path
p = Path(sys.argv[1])
h = dict(Privileged=False, PidMode='', IpcMode='private', UTSMode='', UsernsMode='', CgroupnsMode='private', ReadonlyRootfs=True, CapDrop=['ALL'], CapAdd=None, SecurityOpt=['no-new-privileges:true'], Devices=[], DeviceRequests=None, DeviceCgroupRules=None, PortBindings={}, PublishAllPorts=False, Binds=None, VolumesFrom=None, Memory=268435456, PidsLimit=128, CpusetCpus='0', NetworkMode='none')
c = dict(Name='/test', HostConfig=h, Config=dict(User='65532:65532', ExposedPorts=None), NetworkSettings=dict(Networks={'none': {}}), Mounts=[])
for name in ('a','b'): (p/name).write_text(json.dumps([c]))
PY
# The variables are intentionally expanded by the generated mock, not here.
# shellcheck disable=SC2016
printf '%s\n' '#!/usr/bin/env bash' '[[ "$1" == inspect ]] || exit 2' 'command cat "$FIXTURES/$2"' > "$tmp/bin/docker"
chmod +x "$tmp/bin/docker"
export PATH="$tmp/bin:$PATH" FIXTURES="$tmp"
"$root/scripts/inspect-isolation.sh" a b >/dev/null
for field in Privileged ReadonlyRootfs PidMode IpcMode UTSMode UsernsMode CgroupnsMode CapDrop CapAdd SecurityOpt Devices DeviceRequests DeviceCgroupRules PortBindings PublishAllPorts Binds VolumesFrom Memory PidsLimit CpusetCpus NetworkMode; do
  cp "$tmp/a" "$tmp/saved"
  python3 - "$tmp/a" "$field" <<'PY'
import json, sys
p, key = sys.argv[1:]; c = json.load(open(p)); h = c[0]['HostConfig']
values = dict(Privileged=True, ReadonlyRootfs=False, PidMode='host', IpcMode='host', UTSMode='host', UsernsMode='host', CgroupnsMode='host', CapDrop=[], CapAdd=['SYS_ADMIN'], SecurityOpt=[], Devices=[{}], DeviceRequests=[{}], DeviceCgroupRules=['a'], PortBindings={'80/tcp': [{}]}, PublishAllPorts=True, Binds=['/:/host'], VolumesFrom=['x'], Memory=0, PidsLimit=0, CpusetCpus='', NetworkMode='host')
h[key] = values[key]; open(p, 'w').write(json.dumps(c))
PY
  if "$root/scripts/inspect-isolation.sh" a b >/dev/null 2>&1; then echo "accepted $field" >&2; exit 1; fi
  mv "$tmp/saved" "$tmp/a"
done
cp "$tmp/a" "$tmp/saved"
python3 - "$tmp/a" <<'PY'
import json,sys
p=sys.argv[1]; c=json.load(open(p)); c[0]['HostConfig']['SecurityOpt'].append('seccomp=unconfined'); open(p,'w').write(json.dumps(c))
PY
if "$root/scripts/inspect-isolation.sh" a b >/dev/null 2>&1; then echo 'accepted unconfined seccomp' >&2; exit 1; fi
mv "$tmp/saved" "$tmp/a"
python3 - "$tmp/a" <<'PY'
import json,sys
p=sys.argv[1]; c=json.load(open(p)); c[0]['Mounts']=[dict(Type='bind',Source='/',Destination='/host',RW=False)]; open(p,'w').write(json.dumps(c))
PY
if "$root/scripts/inspect-isolation.sh" a b >/dev/null 2>&1; then echo 'accepted bind' >&2; exit 1; fi
echo 'PASS mocked isolation policy cases'
