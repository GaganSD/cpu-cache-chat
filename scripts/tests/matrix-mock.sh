#!/usr/bin/env bash
# Test the matrix runner with a fake Docker CLI: qualification controls run
# once per configuration, chat attempts reference the bundle, and summary.jsonl
# records first-attempt success, retries, elapsed time, and phase reached.
# No containers are started and no physical-channel result is claimed.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/ccc-matrix-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/state" "$tmp/evidence" "$tmp/log"
python3 - "$tmp/inspect.json" <<'PY'
import json, sys
h = dict(Privileged=False, PidMode='', IpcMode='private', UTSMode='', UsernsMode='', CgroupnsMode='private', ReadonlyRootfs=True, CapDrop=['ALL'], CapAdd=None, SecurityOpt=['no-new-privileges:true'], Devices=[], DeviceRequests=None, DeviceCgroupRules=None, PortBindings={}, PublishAllPorts=False, Binds=None, VolumesFrom=None, Memory=268435456, PidsLimit=128, CpusetCpus='0', NetworkMode='none')
c = dict(Name='/mock', HostConfig=h, Config=dict(User='65532:65532', ExposedPorts=None), NetworkSettings=dict(Networks={'none': {}}), Mounts=[])
with open(sys.argv[1], 'w') as f: json.dump([c], f)
PY
export MOCK_INSPECT="$tmp/inspect.json"
cat > "$tmp/bin/docker" <<'MOCK'
#!/usr/bin/env bash
set -euo pipefail
cmd="$1"; shift
case "$cmd" in
  info) exit 0 ;;
  image)
    if [[ "${1:-}" == inspect && " $* " == *' --format '* ]]; then echo 'sha256:mockimageid'; fi
    exit 0 ;;
  inspect)
    if [[ "${1:-}" == --format ]]; then
      format="$2"; name="$3"
      [[ -f "$MOCK_STATE/$name" ]] || exit 1
      if [[ "$format" != *'.Mounts'* ]]; then echo 'none|false||private'; fi
    else
      [[ -f "$MOCK_STATE/$1" ]] || exit 1
      cat "$MOCK_INSPECT"
    fi ;;
  create)
    args=" $* "
    while [[ $# -gt 0 ]]; do
      if [[ "$1" == --name ]]; then name="$2"; break; fi
      shift
    done
    printf '%s\n' "$args" > "$MOCK_STATE/$name"
    echo "$name" ;;
  start)
    name="${@: -1}"
    args=$(cat "$MOCK_STATE/$name")
    if [[ "$args" == *' --silent '* ]]; then
      echo silent >> "$MOCK_LOG/counts"
      if [[ "${MOCK_SILENT_FAIL:-0}" == 1 ]]; then
        echo '{"schema_version":1,"role":"B","mode":"Cpu","requested_trials":1,"attempted_trials":1,"completed_trials":1,"requested_bits":32,"trials":[{"outcome":"silent_frame","error":null}]}'
      else
        echo '{"schema_version":1,"role":"B","mode":"Cpu","requested_trials":1,"attempted_trials":1,"completed_trials":1,"requested_bits":32,"trials":[{"outcome":"silent_no_frame","error":null}]}'
      fi
    elif [[ "$args" == *' probe '* && "$args" == *' --role a '* ]]; then
      echo '{"schema_version":1,"role":"A","mode":"Cpu","requested_bits":32,"sent_bytes":4,"payload_hex":"5a7fa4c9"}'
    elif [[ "$args" == *' probe '* ]]; then
      echo '{"schema_version":1,"role":"B","mode":"Cpu","requested_trials":1,"attempted_trials":1,"completed_trials":1,"requested_bits":32,"trials":[{"outcome":"decoded","error":null,"decoded_hex":"5a7fa4c9","decoded_bytes":4,"crc_valid":true}]}'
    elif [[ "${MOCK_CHAT_RETRANSMIT:-0}" == 1 && "$args" == *' --role a '* ]]; then
      echo 'link: connected'
      echo 'peer: hello-from-b'
      IFS= read -r message
      printf 'queued: %s\nseq 1: transmitting\nseq 1: awaiting ack\nretransmit seq 1 attempt 2\nseq 1: delivered\n' "$message"
      IFS= read -r _
    elif [[ "${MOCK_CHAT_STALL:-0}" == 1 ]]; then
      echo 'link: connecting'
      sleep 3
    else
      echo 'link: connected'
      if [[ "$args" == *' --role a '* ]]; then
        echo 'peer: hello-from-b'
      else
        echo 'peer: hello-from-a'
      fi
      IFS= read -r message
      printf 'queued: %s\nseq 1: transmitting\nseq 1: awaiting ack\nseq 1: delivered\n' "$message"
      IFS= read -r _
    fi ;;
  rm)
    for name in "$@"; do
      [[ "$name" == -* ]] || rm -f "$MOCK_STATE/$name"
    done ;;
  *) echo "unexpected fake Docker command: $cmd $*" >&2; exit 1 ;;
esac
MOCK
chmod +x "$tmp/bin/docker"
export PATH="$tmp/bin:$PATH" MOCK_STATE="$tmp/state" MOCK_LOG="$tmp/log"

# Two passing attempts: controls run exactly once, both attempts reference the
# bundle, and the summary records first-attempt fields.
MOCK_CHAT_SUCCESS=1 CCC_LOAD_NOTE=matrix-note EVIDENCE_DIR="$tmp/evidence" SETTLE_SECONDS=0 \
  "$ROOT/scripts/run-physical-matrix.sh" chat 2 >"$tmp/matrix-output" 2>&1
base=$(find "$tmp/evidence" -mindepth 1 -maxdepth 1 -type d -name 'cpu-chat-*' -print)
[[ -f "$base/controls/manifest.txt" ]]
grep -Fxq 'result=pass' "$base/controls/manifest.txt"
[[ $(grep -c '^silent$' "$tmp/log/counts") == 1 ]]
[[ $(wc -l < "$base/summary.jsonl" | tr -d ' ') == 2 ]]
python3 - "$base/summary.jsonl" <<'PY'
import json, sys
rows = [json.loads(line) for line in open(sys.argv[1], encoding='utf-8')]
assert [r['attempt'] for r in rows] == [1, 2], rows
for row in rows:
    assert row['exit'] == 0, row
    assert row['first_attempt_success'] is True, row
    assert row['retries'] == 0, row
    assert row['phase_reached'] == 'verify', row
    assert isinstance(row['elapsed_seconds'], int), row
PY
for i in 1 2; do
  rundir=$(find "$base/attempt-$i" -mindepth 1 -maxdepth 1 -type d -name 'ccc-*' -print)
  grep -Fxq 'controls=bundle' "$rundir/run.txt"
  grep -Fxq "controls_bundle=$base/controls" "$rundir/run.txt"
  grep -Eq '^phase=verify ts=[0-9]+$' "$base/attempt-$i/harness.log"
done
grep -Fq 'First-attempt successes (passed with no observed retransmit): 2/2' "$tmp/matrix-output"
grep -Fq 'Eventual passes: 2/2' "$tmp/matrix-output"

# A sender retransmit still passes eventually but is not a first-attempt
# success: the all-pass gate counts eventual passes, and first-attempt
# successes are reported separately.
mkdir -p "$tmp/evidence2"
MOCK_CHAT_RETRANSMIT=1 CCC_LOAD_NOTE=matrix-note EVIDENCE_DIR="$tmp/evidence2" \
  "$ROOT/scripts/run-physical-matrix.sh" chat 1 >"$tmp/retry-output" 2>&1
base2=$(find "$tmp/evidence2" -mindepth 1 -maxdepth 1 -type d -name 'cpu-chat-*' -print)
python3 - "$base2/summary.jsonl" <<'PY'
import json, sys
row, = [json.loads(line) for line in open(sys.argv[1], encoding='utf-8')]
assert row['exit'] == 0, row
assert row['first_attempt_success'] is False, row
assert row['retries'] >= 1, row
PY
grep -Fq 'First-attempt successes (passed with no observed retransmit): 0/1' "$tmp/retry-output"
grep -Fq 'Eventual passes: 1/1' "$tmp/retry-output"

# A failed attempt records the phase reached and makes the matrix exit nonzero.
mkdir -p "$tmp/evidence3"
if MOCK_CHAT_STALL=1 CCC_LOAD_NOTE=matrix-note EVIDENCE_DIR="$tmp/evidence3" \
  "$ROOT/scripts/run-physical-matrix.sh" chat 1 >"$tmp/stall-output" 2>&1; then
  echo 'expected matrix failure' >&2; exit 1
fi
base3=$(find "$tmp/evidence3" -mindepth 1 -maxdepth 1 -type d -name 'cpu-chat-*' -print)
python3 - "$base3/summary.jsonl" <<'PY'
import json, sys
row, = [json.loads(line) for line in open(sys.argv[1], encoding='utf-8')]
assert row['exit'] != 0, row
assert row['first_attempt_success'] is False, row
assert row['phase_reached'] == 'connect', row
PY
grep -Fq 'Eventual passes: 0/1' "$tmp/stall-output"

# Failed qualification controls abort the campaign before any attempt.
mkdir -p "$tmp/evidence4"
if MOCK_SILENT_FAIL=1 EVIDENCE_DIR="$tmp/evidence4" \
  "$ROOT/scripts/run-physical-matrix.sh" chat 1 >"$tmp/abort-output" 2>&1; then
  echo 'expected qualification failure' >&2; exit 1
fi
grep -Fq 'campaign aborted before any chat attempt' "$tmp/abort-output"
base4=$(find "$tmp/evidence4" -mindepth 1 -maxdepth 1 -type d -name 'cpu-chat-*' -print)
[[ ! -e "$base4/attempt-1" && ! -e "$base4/summary.jsonl" ]]

# SLOT_MS=25 passes matrix validation and reaches the controls stage.
mkdir -p "$tmp/evidence5"
MOCK_CHAT_SUCCESS=1 SLOT_MS=25 EVIDENCE_DIR="$tmp/evidence5" \
  "$ROOT/scripts/run-physical-matrix.sh" chat 1 >"$tmp/slot25-output" 2>&1
grep -Fq 'slot=25ms' "$tmp/slot25-output"
if SLOT_MS=0 "$ROOT/scripts/run-physical-matrix.sh" chat 1 >"$tmp/slot0-output" 2>&1; then
  echo 'expected slot validation failure' >&2; exit 1
fi
grep -Fq 'SLOT_MS must be 1..500' "$tmp/slot0-output"
echo 'PASS: matrix controls bundle, summary fields, first-attempt counts, abort gate, slot presets'
