#!/usr/bin/env bash
# Qualification controls for the physical channel: the silent no-sender window
# followed by the receiver-blind probe, run ONCE per immutable configuration
# (image id + channel + slot + cpuset + recorded load note) into a control
# bundle directory. A passing bundle is a single qualification run, not a
# reliability guarantee.
#
# The silent control always runs to completion and its container is removed
# before the probe containers are created; the silent control and an active
# probe never overlap.
#
# Bundle layout:
#   manifest.txt   key=value identity + result (schema_version=1)
#   silent.out/.err, probe-a.out/.err, probe-b.out/.err
# Re-running with a matching manifest validates the stored evidence and reuses
# the bundle instead of re-running the controls.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
CHANNEL="${CHANNEL:-cpu}"
SLOT_MS="${SLOT_MS:-50}"
CPUSET="${CPUSET:-0}"
OUT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      [[ $# -ge 2 ]] || { echo '--out needs a directory' >&2; exit 1; }
      OUT="$2"; shift 2 ;;
    *) echo 'usage: run-controls.sh --out DIR' >&2; exit 1 ;;
  esac
done
[[ -n "$OUT" ]] || { echo 'usage: run-controls.sh --out DIR' >&2; exit 1; }
[[ "$CHANNEL" == cpu || "$CHANNEL" == cache ]] || { echo 'CHANNEL must be cpu or cache' >&2; exit 1; }
if [[ ! "$SLOT_MS" =~ ^[0-9]{1,3}$ ]] || (( 10#$SLOT_MS < 1 || 10#$SLOT_MS > 500 )); then
  echo 'SLOT_MS must be 1..500' >&2; exit 1
fi
SLOT_MS=$((10#$SLOT_MS))
# Three complete empty protocol control frame windows, rounded up to seconds.
SILENT_SECONDS=$(((3 * (156 + 13 * 19) * SLOT_MS + 999) / 1000))
LOAD_NOTE=$(printf '%s' "${CCC_LOAD_NOTE:-unspecified}" | tr '\n' ' ')
umask 077
mkdir -p -- "$OUT"
manifest="$OUT/manifest.txt"
log_phase() {
  local ts
  ts=$(date +%s)
  echo "phase=$1 ts=$ts"
  if [[ -n "${CCC_PHASES_LOG:-}" ]]; then printf 'phase=%s ts=%s\n' "$1" "$ts" >> "$CCC_PHASES_LOG"; fi
}
sha256_of() { printf '%s' "$1" | { sha256sum 2>/dev/null || shasum -a 256; } | awk '{print $1}'; }
docker info >/dev/null
docker image inspect "$IMAGE" >/dev/null
image_id=$(docker image inspect "$IMAGE" --format '{{.Id}}')
[[ -n "$image_id" ]] || { echo "FAIL: cannot resolve image id for $IMAGE" >&2; exit 1; }
# Keep this canonical string in sync with scripts/e2e-docker.sh.
config_sha=$(sha256_of "ccc-controls-v1|$image_id|$CHANNEL|$SLOT_MS|$CPUSET|$LOAD_NOTE")
bundle_valid() {
  [[ -f "$manifest" && -f "$OUT/silent.out" && -f "$OUT/probe-a.out" && -f "$OUT/probe-b.out" ]] || return 1
  [[ $(sed -n 's/^config_sha256=//p' "$manifest" 2>/dev/null) == "$config_sha" ]] || return 1
  python3 "$ROOT/scripts/probe-report.py" bundle "$OUT" >/dev/null
}
if bundle_valid; then
  echo "Qualification controls: reusing bundle $OUT (config ${config_sha:0:12}...); single qualification run, not a reliability guarantee"
  exit 0
fi
prefix="ccc-controls-$$"
a="$prefix-a"; b="$prefix-b"
owned_a=0; owned_b=0
cleanup() {
  if [[ "$owned_a" == 1 ]]; then docker rm -f "$a" >/dev/null 2>&1 || true; fi
  if [[ "$owned_b" == 1 ]]; then docker rm -f "$b" >/dev/null 2>&1 || true; fi
  local pid
  for pid in "${probe_a:-}" "${probe_b:-}"; do
    [[ -z "$pid" ]] || wait "$pid" 2>/dev/null || true
  done
}
trap cleanup EXIT
for name in "$a" "$b"; do
  if docker inspect "$name" >/dev/null 2>&1; then
    echo "$name already exists; refusing to remove it" >&2
    exit 1
  fi
done
common=(--init --network none --ipc private --cgroupns private --read-only
  --tmpfs '/tmp:rw,nosuid,size=64m' --security-opt no-new-privileges
  --cap-drop ALL --user 65532:65532 --memory 256m --pids-limit 128 --cpuset-cpus "$CPUSET")
if [[ "${CCC_DEBUG:-}" == 1 ]]; then common+=(--env CCC_DEBUG=1); fi

# A control with no sender must time out without decoding a frame. The report
# remains diagnostic; this alone cannot establish a working transport.
log_phase silent
echo "Silent no-sender control: ${SILENT_SECONDS}s (three nominal empty control-frame windows); unrelated-load control not run."
docker create --name "$b" "${common[@]}" "$IMAGE" probe --role b --channel "$CHANNEL" \
  --slot-ms "$SLOT_MS" --silent --silent-seconds "$SILENT_SECONDS" >/dev/null
owned_b=1
docker start -a "$b" >"$OUT/silent.out" 2>"$OUT/silent.err" || { echo 'FAIL: silent control process failed'; exit 1; }
cat "$OUT/silent.out"
if ! python3 "$ROOT/scripts/probe-report.py" silent "$OUT/silent.out"; then
  echo 'FAIL: silent sender control did not pass'; exit 1
fi
docker rm "$b" >/dev/null
owned_b=0

# Probe receives without knowing the payload. Compare logs only after both
# processes exit; no expected bytes are passed to the receiver. The silent
# control container is gone before the probe starts: no overlap.
log_phase probe
docker create --name "$b" "${common[@]}" "$IMAGE" probe --role b --channel "$CHANNEL" --bits 32 --slot-ms "$SLOT_MS" >/dev/null
owned_b=1
docker create --name "$a" "${common[@]}" "$IMAGE" probe --role a --channel "$CHANNEL" --bits 32 --slot-ms "$SLOT_MS" >/dev/null
owned_a=1
"$ROOT/scripts/inspect-isolation.sh" "$a" "$b"
docker start -a "$b" >"$OUT/probe-b.out" 2>"$OUT/probe-b.err" & probe_b=$!
sleep 2
docker start -a "$a" >"$OUT/probe-a.out" 2>"$OUT/probe-a.err" & probe_a=$!
status=0
wait "$probe_a" || status=1
wait "$probe_b" || status=1
cat "$OUT/probe-a.out" "$OUT/probe-b.out"
[[ "$status" == 0 ]] || { echo 'FAIL: probe process failed'; exit 1; }
if ! python3 "$ROOT/scripts/probe-report.py" compare "$OUT/probe-a.out" "$OUT/probe-b.out"; then
  echo 'FAIL: receiver-blind probe did not decode all expected bits; BER unavailable if no frame decoded'; exit 1
fi
echo 'PASS: probe decoded expected bytes (0 bit errors in this frame) and silent control passed; single qualification run, not a reliability guarantee'
docker rm "$a" "$b" >/dev/null
owned_a=0; owned_b=0
load_avg=$({ if [[ -r /proc/loadavg ]]; then cut -d' ' -f1-3 < /proc/loadavg; else sysctl -n vm.loadavg 2>/dev/null || echo unknown; fi; } | tr '\n' ' ')
{
  printf 'schema_version=1\n'
  printf 'created_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  printf 'image=%s\n' "$IMAGE"
  printf 'image_id=%s\n' "$image_id"
  printf 'channel=%s\n' "$CHANNEL"
  printf 'slot_ms=%s\n' "$SLOT_MS"
  printf 'cpuset=%s\n' "$CPUSET"
  printf 'silent_seconds=%s\n' "$SILENT_SECONDS"
  printf 'load_note=%s\n' "$LOAD_NOTE"
  printf 'load_avg=%s\n' "$load_avg"
  printf 'host_uname=%s\n' "$(uname -srm)"
  printf 'config_sha256=%s\n' "$config_sha"
  printf 'result=pass\n'
  printf 'note=single qualification run; not a reliability guarantee\n'
} > "$manifest"
echo "Control bundle written: $OUT (config ${config_sha:0:12}...)"
