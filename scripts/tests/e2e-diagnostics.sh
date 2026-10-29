#!/usr/bin/env bash
# Test evidence retention, bounded deadlines, and control-bundle reuse using a
# fake Docker CLI, without starting containers or claiming a physical-channel
# result.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
tmp=$(mktemp -d "${TMPDIR:-/tmp}/ccc-e2e-diagnostics-test.XXXXXX")
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/state" "$tmp/evidence" "$tmp/transient" "$tmp/log"
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
    # An unterminated diagnostic would corrupt the next stdout line if the
    # harness merged streams into one file.
    printf 'mock debug without newline' >&2
    if [[ "$args" == *' --silent '* ]]; then
      echo silent >> "$MOCK_LOG/counts"
      if [[ "${MOCK_SILENT_FAIL:-0}" == 1 ]]; then
        echo '{"schema_version":1,"role":"B","mode":"Cpu","requested_trials":1,"attempted_trials":1,"completed_trials":1,"requested_bits":32,"trials":[{"outcome":"silent_frame","error":null}]}'
      else
        echo '{"schema_version":1,"role":"B","mode":"Cpu","requested_trials":1,"attempted_trials":1,"completed_trials":1,"requested_bits":32,"trials":[{"outcome":"silent_no_frame","error":null}]}'
      fi
    elif [[ "$args" == *' probe '* && "$args" == *' --role a '* ]]; then
      echo probe-a >> "$MOCK_LOG/counts"
      echo '{"schema_version":1,"role":"A","mode":"Cpu","requested_bits":32,"sent_bytes":4,"payload_hex":"5a7fa4c9"}'
    elif [[ "$args" == *' probe '* ]]; then
      echo probe-b >> "$MOCK_LOG/counts"
      echo '{"schema_version":1,"role":"B","mode":"Cpu","requested_trials":1,"attempted_trials":1,"completed_trials":1,"requested_bits":32,"trials":[{"outcome":"decoded","error":null,"decoded_hex":"5a7fa4c9","decoded_bytes":4,"crc_valid":true}]}'
    elif [[ "${MOCK_CHAT_SUCCESS:-0}" == 1 ]]; then
      echo 'link: connected'
      if [[ "$args" == *' --role a '* ]]; then
        echo 'peer: hello-from-b'
      else
        echo 'peer: hello-from-a'
      fi
      IFS= read -r message
      printf 'queued: %s\nseq 1: transmitting\nseq 1: awaiting ack\nseq 1: delivered\n' "$message"
      IFS= read -r _
    else
      echo 'link: connecting'
      sleep 3
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
count_silent() { grep -c '^silent$' "$tmp/log/counts" 2>/dev/null || true; }

# A failed silent control must leave the exact output and exit status in a new
# private evidence directory, and still remove the fake container.
if MOCK_SILENT_FAIL=1 EVIDENCE_DIR="$tmp/evidence" WAIT_SECONDS=1 \
  "$ROOT/scripts/e2e-docker.sh" >"$tmp/silent-output" 2>&1; then
  echo 'expected silent control failure' >&2; exit 1
fi
run1=$(find "$tmp/evidence" -mindepth 1 -maxdepth 1 -type d -print)
[[ -n "$run1" && -f "$run1/controls/silent.out" ]]
grep -Fq '"outcome":"silent_frame"' "$run1/controls/silent.out"
grep -Fxq 'mock debug without newline' "$run1/controls/silent.err"
grep -Fq 'mock debug without newline' "$tmp/silent-output"
grep -Fxq 'exit_status=1' "$run1/run.txt"
grep -Fxq 'phase_reached=silent' "$run1/run.txt"
grep -Eq '^phase=start ts=[0-9]+$' "$tmp/silent-output"
grep -Eq '^phase=silent ts=[0-9]+$' "$tmp/silent-output"
[[ -z "$(ls -A "$tmp/state")" ]]

# A successful gate followed by a missing handshake uses the requested short
# attempt deadline, retains both complete logs, and removes the FIFOs and
# containers. The deadline failure names the phase.
if EVIDENCE_DIR="$tmp/evidence" WAIT_SECONDS=1 \
  "$ROOT/scripts/e2e-docker.sh" >"$tmp/handshake-output" 2>&1; then
  echo 'expected handshake failure' >&2; exit 1
fi
[[ $(find "$tmp/evidence" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ') == 2 ]]
run2=$(find "$tmp/evidence" -mindepth 1 -maxdepth 1 -type d ! -path "$run1" -print)
grep -Fxq 'link: connecting' "$run2/a.out"
grep -Fxq 'link: connecting' "$run2/b.out"
grep -Fxq 'mock debug without newline' "$run2/a.err"
grep -Fxq 'mock debug without newline' "$run2/b.err"
grep -Fq 'mock debug without newline' "$tmp/handshake-output"
grep -Fq "FAIL: attempt deadline expired during phase connect waiting for 'link: connected'" "$tmp/handshake-output"
grep -Fxq 'wait_seconds=1' "$run2/run.txt"
grep -Fxq 'exit_status=1' "$run2/run.txt"
grep -Fxq 'phase_reached=connect' "$run2/run.txt"
grep -Fxq 'controls=inline' "$run2/run.txt"
[[ ! -e "$run2/a.in" && ! -e "$run2/b.in" && -z "$(ls -A "$tmp/state")" ]]
grep -Fq "Evidence retained: $run2" "$tmp/handshake-output"

# Check assertions on a successful mock chat while every container emits an
# unterminated debug line. Both state lines and probe payload comparisons must
# remain intact, each stream must remain available in evidence, and all six
# phase events must appear in order.
MOCK_CHAT_SUCCESS=1 EVIDENCE_DIR="$tmp/evidence" WAIT_SECONDS=2 \
  "$ROOT/scripts/e2e-docker.sh" >"$tmp/success-output" 2>&1
run3=$(find "$tmp/evidence" -mindepth 1 -maxdepth 1 -type d ! -path "$run1" ! -path "$run2" -print)
grep -Fxq 'seq 1: transmitting' "$run3/a.out"
grep -Fxq 'seq 1: transmitting' "$run3/b.out"
grep -Fxq 'mock debug without newline' "$run3/controls/probe-a.err"
grep -Fxq 'mock debug without newline' "$run3/controls/probe-b.err"
grep -Fxq 'mock debug without newline' "$run3/a.err"
grep -Fxq 'mock debug without newline' "$run3/b.err"
grep -Fq 'PASS docker cpu chat both directions' "$tmp/success-output"
grep -Fxq 'exit_status=0' "$run3/run.txt"
grep -Fxq 'controls=inline' "$run3/run.txt"
grep -Fxq "controls_bundle=$run3/controls" "$run3/run.txt"
grep -Fxq 'phase_reached=verify' "$run3/run.txt"
grep -Fxq 'result=pass' "$run3/controls/manifest.txt"
sed -n 's/^phase=\([a-z]*\) ts=.*/\1/p' "$tmp/success-output" | tr '\n' ' ' > "$tmp/phases"
grep -Fxq 'start silent probe connect chat verify ' "$tmp/phases"

# A valid control bundle is referenced, not re-run: the silent and probe
# counts stay flat, the manifest is copied into the attempt evidence, and
# run.txt records the bundle reference. A bundle whose recorded load note does
# not match the current configuration is ignored and controls run inline.
CCC_LOAD_NOTE=diag-note "$ROOT/scripts/run-controls.sh" --out "$tmp/bundle" >"$tmp/controls-output" 2>&1
grep -Fxq 'result=pass' "$tmp/bundle/manifest.txt"
grep -Fq 'not a reliability guarantee' "$tmp/controls-output"
before=$(count_silent)
MOCK_CHAT_SUCCESS=1 CCC_LOAD_NOTE=diag-note EVIDENCE_DIR="$tmp/evidence" WAIT_SECONDS=2 \
  "$ROOT/scripts/e2e-docker.sh" --controls-dir "$tmp/bundle" >"$tmp/bundle-output" 2>&1
[[ $(count_silent) == "$before" ]]
run4=$(find "$tmp/evidence" -mindepth 1 -maxdepth 1 -type d ! -path "$run1" ! -path "$run2" ! -path "$run3" -print)
[[ -f "$run4/controls-manifest.txt" && ! -e "$run4/controls" ]]
grep -Fxq 'controls=bundle' "$run4/run.txt"
grep -Fxq "controls_bundle=$tmp/bundle" "$run4/run.txt"
grep -Fq "Qualification controls: referencing bundle $tmp/bundle" "$tmp/bundle-output"
sed -n 's/^phase=\([a-z]*\) ts=.*/\1/p' "$tmp/bundle-output" | tr '\n' ' ' > "$tmp/bundle-phases"
grep -Fxq 'start connect chat verify ' "$tmp/bundle-phases"
MOCK_CHAT_SUCCESS=1 CCC_LOAD_NOTE=other-note CCC_CONTROLS_DIR="$tmp/bundle" EVIDENCE_DIR="$tmp/evidence" WAIT_SECONDS=2 \
  "$ROOT/scripts/e2e-docker.sh" >"$tmp/mismatch-output" 2>&1
[[ $(count_silent) == $((before + 1)) ]]
grep -Fq 'no valid bundle for this configuration' "$tmp/mismatch-output"
run5=$(find "$tmp/evidence" -mindepth 1 -maxdepth 1 -type d ! -path "$run1" ! -path "$run2" ! -path "$run3" ! -path "$run4" -print)
grep -Fxq 'controls=inline' "$run5/run.txt"

# Without opt-in evidence, temporary output is removed. Invalid wait values
# are rejected before Docker is invoked.
if MOCK_SILENT_FAIL=1 TMPDIR="$tmp/transient" WAIT_SECONDS=1 \
  "$ROOT/scripts/e2e-docker.sh" >"$tmp/transient-output" 2>&1; then
  echo 'expected silent control failure' >&2; exit 1
fi
[[ -z "$(ls -A "$tmp/transient")" ]]
if WAIT_SECONDS=0 "$ROOT/scripts/e2e-docker.sh" >"$tmp/invalid-output" 2>&1; then
  echo 'expected invalid wait failure' >&2; exit 1
fi
grep -Fq 'WAIT_SECONDS must be an integer' "$tmp/invalid-output"
echo 'PASS: split streams, delivery assertions, phases, deadlines, control bundles, evidence, cleanup, and configurable wait'
