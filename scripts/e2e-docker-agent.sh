#!/usr/bin/env bash
# Agent-versus-agent game over the physical channel in two containers.
#
# Isolation notes, honestly stated:
# - Game moves, acks, and turn coordination travel ONLY over the selected
#   physical channel; the harness passes no data between the containers.
# - Hosted inference uses separate egress-enabled bridges. These are not
#   destination-restricted firewalls; live peer isolation is unverified.
# - The API key is read from the operator's environment and passed via
#   --env; it is never written into the image or the evidence directory.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:agent}"
CHANNEL="${CHANNEL:-cpu}"
SLOT_MS="${SLOT_MS:-50}"
CPUSET="${CPUSET:-0}"
WAIT_SECONDS="${WAIT_SECONDS:-3600}"
[[ "$CHANNEL" == cpu || "$CHANNEL" == cache ]] || { echo 'CHANNEL must be cpu or cache' >&2; exit 1; }
export CCC_AGENT_PROVIDER="${CCC_AGENT_PROVIDER:-opencode-go}"
if [[ "$CCC_AGENT_PROVIDER" != opencode-go ]]; then
  echo 'Offline/other providers are unsupported: local runtime and weights are not provisioned.' >&2; exit 1
fi
if [[ "${CCC_HOSTED_OPT_IN:-}" != 1 ]]; then
  echo 'Set CCC_HOSTED_OPT_IN=1 to allow egress-enabled hosted inference; board prompts are sent to the provider and charges may apply.' >&2; exit 1
fi
if [[ -z "${OPENCODE_GO_API_KEY:-}" ]]; then
  echo 'OPENCODE_GO_API_KEY must be set for CCC_AGENT_PROVIDER=opencode-go' >&2
  exit 1
fi
umask 077
if [[ -n "${EVIDENCE_DIR:-}" ]]; then
  mkdir -p -- "$EVIDENCE_DIR"
  workdir=$(mktemp -d "$EVIDENCE_DIR/ccc-dockeragent.XXXXXX")
  retain_evidence=1
  echo "Evidence directory: $workdir"
else
  workdir=$(mktemp -d "${TMPDIR:-/tmp}/ccc-dockeragent.XXXXXX")
  retain_evidence=0
fi
prefix="ccc-agent-$$"
a="$prefix-a"; b="$prefix-b"
net_a="$prefix-net-a"; net_b="$prefix-net-b"
owned_a=0; owned_b=0; owned_net_a=0; owned_net_b=0
repo_head=unknown; repo_dirty=unknown
# Invoked by the EXIT trap.
# EXIT trap callback; ShellCheck 0.9/0.11 disagree about reachability.
# shellcheck disable=SC2317,SC2329
cleanup() {
  local result=$? pid
  exec 3>&- 4>&- || true
  if [[ "$owned_a" == 1 ]]; then docker rm -f "$a" >/dev/null 2>&1 || true; fi
  if [[ "$owned_b" == 1 ]]; then docker rm -f "$b" >/dev/null 2>&1 || true; fi
  if [[ "$owned_net_a" == 1 ]]; then docker network rm "$net_a" >/dev/null 2>&1 || true; fi
  if [[ "$owned_net_b" == 1 ]]; then docker network rm "$net_b" >/dev/null 2>&1 || true; fi
  for pid in "${A_PID:-}" "${B_PID:-}"; do
    [[ -z "$pid" ]] || wait "$pid" 2>/dev/null || true
  done
  if [[ "$result" != 0 ]]; then
    local log
    for log in a.out a.err b.out b.err; do
      if [[ -f "$workdir/$log" ]]; then
        printf '%s\n' "----- $log (last 30 lines) -----" >&2
        tail -30 "$workdir/$log" >&2 || true
      fi
    done
  fi
  if [[ "$retain_evidence" == 1 ]]; then
    rm -f "$workdir/a.in" "$workdir/b.in"
    {
      printf 'exit_status=%s\nchannel=%s\nmode=game --agent\nslot_ms=%s\ncpuset=%s\nwait_seconds=%s\nimage=%s\nprovider=%s\nmodel=%s\n' \
        "$result" "$CHANNEL" "$SLOT_MS" "$CPUSET" "$WAIT_SECONDS" "$IMAGE" \
        "$CCC_AGENT_PROVIDER" "${CCC_AGENT_MODEL:-glm-5.3-flash}"
      printf 'image_digest=%s\n' "$(docker image inspect "$IMAGE" --format '{{.Id}}' 2>/dev/null || echo unknown)"
      printf 'repo_head=%s\n' "$repo_head"
      printf 'repo_dirty=%s\n' "$repo_dirty"
      printf 'ccc_debug=%s\nccc_agent_debug=%s\n' "${CCC_DEBUG:-0}" "${CCC_AGENT_DEBUG:-0}"
      printf 'host_uname=%s\n' "$(uname -srm)"
      printf 'date=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    } > "$workdir/run.txt"
    echo "Evidence retained: $workdir"
  else
    rm -rf "$workdir"
  fi
}
trap cleanup EXIT
for name in "$a" "$b"; do
  if docker inspect "$name" >/dev/null 2>&1; then
    echo "$name already exists; refusing to remove it" >&2
    exit 1
  fi
done
docker info >/dev/null
docker image inspect "$IMAGE" >/dev/null
# Provenance at launch, not cleanup.
repo_head=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)
repo_dirty=$(git -C "$ROOT" status --porcelain 2>/dev/null | grep -q . && echo yes || echo no)
# Separate bridge networks: egress to the inference API for each player, but
# no layer-2 or routed path between the two containers.
docker network create "$net_a" >/dev/null; owned_net_a=1
docker network create "$net_b" >/dev/null; owned_net_b=1
common=(--init --ipc private --cgroupns private --read-only
  --tmpfs '/tmp:rw,nosuid,size=64m' --security-opt no-new-privileges
  --cap-drop ALL --user 65532:65532 --memory 768m --pids-limit 128 --cpuset-cpus "$CPUSET"
  --env CCC_AGENT_PROVIDER --env OPENCODE_GO_API_KEY)
if [[ -n "${CCC_AGENT_MODEL:-}" ]]; then common+=(--env CCC_AGENT_MODEL); fi
if [[ "${CCC_DEBUG:-}" == 1 ]]; then common+=(--env CCC_DEBUG=1); fi
if [[ "${CCC_AGENT_DEBUG:-}" == 1 ]]; then common+=(--env CCC_AGENT_DEBUG=1); fi

mkfifo "$workdir/a.in" "$workdir/b.in"
exec 3<>"$workdir/a.in" 4<>"$workdir/b.in"
docker create -i --name "$a" --network "$net_a" "${common[@]}" "$IMAGE" \
  game --role a --channel "$CHANNEL" --plain --slot-ms "$SLOT_MS" --agent >/dev/null
owned_a=1
docker create -i --name "$b" --network "$net_b" "${common[@]}" "$IMAGE" \
  game --role b --channel "$CHANNEL" --plain --slot-ms "$SLOT_MS" --agent >/dev/null
owned_b=1
# Show and assert the actual network policy: exactly one network each, disjoint.
nets_a=$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$a")
nets_b=$(docker inspect -f '{{range $k,$v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$b")
echo "container networks: A=[$nets_a] B=[$nets_b] (must be disjoint, no host, no none-shared)"
[[ "${nets_a% }" == "$net_a" && "${nets_b% }" == "$net_b" ]] || {
  echo 'FAIL: unexpected container network membership' >&2; exit 1; }
if docker inspect "$a" "$b" --format '{{.HostConfig.Privileged}} {{.HostConfig.NetworkMode}}' | grep -q true; then
  echo 'FAIL: privileged container' >&2; exit 1
fi
"$ROOT/scripts/inspect-isolation.sh" --hosted "$a" "$b" > "$workdir/policy.txt"
docker start -a -i "$a" <"$workdir/a.in" >"$workdir/a.out" 2>"$workdir/a.err" & A_PID=$!
docker start -a -i "$b" <"$workdir/b.in" >"$workdir/b.out" 2>"$workdir/b.err" & B_PID=$!
sleep 2
# Capture the RUNTIME network state after start, not just the configured one.
{
  echo "container networks (configured): A=[$nets_a] B=[$nets_b]"
  docker inspect "$a" "$b" --format '{{.Name}} state={{.State.Status}} image={{.Image}} networkmode={{.HostConfig.NetworkMode}} networks={{json .NetworkSettings.Networks}} privileged={{.HostConfig.Privileged}} cpuset={{.HostConfig.CpusetCpus}} ipc={{.HostConfig.IpcMode}} pid={{.HostConfig.PidMode}} readonly={{.HostConfig.ReadonlyRootfs}} mounts={{json .Mounts}}'
  docker network inspect "$net_a" "$net_b" --format '{{.Name}} internal={{.Internal}} containers={{json .Containers}}'
  docker image inspect "$IMAGE" --format 'image created={{.Created}} id={{.Id}}'
} > "$workdir/docker-inspect.txt"
echo 'UNVERIFIED: no known-listener positive/negative live network controls were run.' >&2
wait_for() {
  local file="$1" needle="$2" tries="${3:-$WAIT_SECONDS}"
  for _ in $(seq 1 "$tries"); do
    if grep -Fq -- "$needle" "$file"; then return 0; fi
    if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
    sleep 1
  done
  echo "FAIL: waiting for '$needle' in $file" >&2
  return 1
}
wait_for "$workdir/a.out" 'link: connected' 900
wait_for "$workdir/b.out" 'link: connected' 900
echo 'PASS: both agents connected over the physical channel'
outcome=""
for _ in $(seq 1 "$WAIT_SECONDS"); do
  if grep -qE "X wins|O wins|draw" "$workdir/a.out" && grep -qE "X wins|O wins|draw" "$workdir/b.out"; then
    outcome="finished"; break
  fi
  if grep -q "agent paused" "$workdir/a.out" || grep -q "agent paused" "$workdir/b.out"; then
    outcome="agent-paused"; break
  fi
  if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
  sleep 1
done
if [[ "$outcome" != finished ]]; then
  echo "FAIL: physical agent game did not finish (outcome=${outcome:-timeout})" >&2
  exit 1
fi
a_result=$(grep -oE "X wins|O wins|draw" "$workdir/a.out" | tail -1)
b_result=$(grep -oE "X wins|O wins|draw" "$workdir/b.out" | tail -1)
[[ "$a_result" == "$b_result" ]] || { echo "FAIL: outcome mismatch A=$a_result B=$b_result" >&2; exit 1; }
grep -q "agent chose" "$workdir/a.out" || { echo 'FAIL: A never recorded an agent move' >&2; exit 1; }
grep -q "agent chose" "$workdir/b.out" || { echo 'FAIL: B never recorded an agent move' >&2; exit 1; }
moves_a=$(grep -c 'agent chose' "$workdir/a.out")
moves_b=$(grep -c 'agent chose' "$workdir/b.out")
echo "UNVERIFIED physical $CHANNEL agent game: matching outcome text $a_result; boards and live network policy not verified (agent moves A=$moves_a B=$moves_b)"
printf '/q\n' >&3; printf '/q\n' >&4
wait "$A_PID" "$B_PID"
exit 1
