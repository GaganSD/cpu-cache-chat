#!/usr/bin/env bash
# Headless isolated two-container tic-tac-toe over the physical channel.
# No host data is passed between peers; evidence stays host-side.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="${IMAGE:-cpu-cache-chat:local}"
CHANNEL="${CHANNEL:-cpu}"
SLOT_MS="${SLOT_MS:-50}"
CPUSET="${CPUSET:-0}"
WAIT_SECONDS="${WAIT_SECONDS:-480}"
[[ "$CHANNEL" == cpu || "$CHANNEL" == cache ]] || { echo 'CHANNEL must be cpu or cache' >&2; exit 1; }
[[ "$WAIT_SECONDS" =~ ^[0-9]{1,4}$ ]] || { echo 'WAIT_SECONDS must be an integer from 1 to 3600' >&2; exit 1; }
WAIT_SECONDS=$((10#$WAIT_SECONDS))
(( WAIT_SECONDS >= 1 && WAIT_SECONDS <= 3600 )) || { echo 'WAIT_SECONDS must be an integer from 1 to 3600' >&2; exit 1; }
WAIT_TRIES=$((WAIT_SECONDS * 4))
umask 077
if [[ -n "${EVIDENCE_DIR:-}" ]]; then
  mkdir -p -- "$EVIDENCE_DIR"
  workdir=$(mktemp -d "$EVIDENCE_DIR/ccc-dockergame.XXXXXX")
  retain_evidence=1
  echo "Evidence directory: $workdir"
else
  workdir=$(mktemp -d "${TMPDIR:-/tmp}/ccc-dockergame.XXXXXX")
  retain_evidence=0
fi
prefix="ccc-game-$$"
a="$prefix-a"; b="$prefix-b"
owned_a=0; owned_b=0
cleanup() {
  local result=$? pid
  exec 3>&- 4>&- || true
  if [[ "$owned_a" == 1 ]]; then docker rm -f "$a" >/dev/null 2>&1 || true; fi
  if [[ "$owned_b" == 1 ]]; then docker rm -f "$b" >/dev/null 2>&1 || true; fi
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
      printf 'exit_status=%s\nchannel=%s\nmode=game\nslot_ms=%s\ncpuset=%s\nwait_seconds=%s\nimage=%s\n' \
        "$result" "$CHANNEL" "$SLOT_MS" "$CPUSET" "$WAIT_SECONDS" "$IMAGE"
      printf 'image_digest=%s\n' "$(docker image inspect "$IMAGE" --format '{{.Id}}' 2>/dev/null || echo unknown)"
      printf 'repo_head=%s\n' "$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
      printf 'repo_dirty=%s\n' "$(git -C "$ROOT" status --porcelain 2>/dev/null | grep -q . && echo yes || echo no)"
      printf 'ccc_debug=%s\n' "${CCC_DEBUG:-0}"
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
common=(--init --network none --ipc private --read-only
  --tmpfs '/tmp:rw,nosuid,size=64m' --security-opt no-new-privileges
  --cap-drop ALL --user 65532:65532 --memory 256m --pids-limit 128 --cpuset-cpus "$CPUSET")
if [[ "${CCC_DEBUG:-}" == 1 ]]; then common+=(--env CCC_DEBUG=1); fi

mkfifo "$workdir/a.in" "$workdir/b.in"
exec 3<>"$workdir/a.in" 4<>"$workdir/b.in"
docker create -i --name "$a" "${common[@]}" "$IMAGE" game --role a --channel "$CHANNEL" --plain --slot-ms "$SLOT_MS" >/dev/null
owned_a=1
docker create -i --name "$b" "${common[@]}" "$IMAGE" game --role b --channel "$CHANNEL" --plain --slot-ms "$SLOT_MS" >/dev/null
owned_b=1
"$ROOT/scripts/inspect-isolation.sh" "$a" "$b" | tee "$workdir/inspect-isolation.txt"
docker start -a -i "$a" <"$workdir/a.in" >"$workdir/a.out" 2>"$workdir/a.err" & A_PID=$!
docker start -a -i "$b" <"$workdir/b.in" >"$workdir/b.out" 2>"$workdir/b.err" & B_PID=$!
sleep 2
# Retain the actual runtime configuration the verdict relies on, after start.
docker inspect "$a" "$b" --format '{{.Name}} state={{.State.Status}} network={{.HostConfig.NetworkMode}} privileged={{.HostConfig.Privileged}} cpuset={{.HostConfig.CpusetCpus}} ipc={{.HostConfig.IpcMode}} pid={{.HostConfig.PidMode}} readonly={{.HostConfig.ReadonlyRootfs}} mounts={{json .Mounts}}' > "$workdir/docker-inspect.txt"
wait_for() {
  local file="$1" needle="$2" tries="${3:-$WAIT_TRIES}"
  for _ in $(seq 1 "$tries"); do
    if grep -Fq -- "$needle" "$file"; then return 0; fi
    if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
    sleep 0.25
  done
  echo "FAIL: waiting for '$needle' in $file" >&2
  return 1
}
wait_absent() {
  # Give the link a bounded window in which a forbidden line must not appear.
  local file="$1" needle="$2" window_seconds="$3"
  local end=$((SECONDS + window_seconds))
  while (( SECONDS < end )); do
    if grep -Fq -- "$needle" "$file"; then
      echo "FAIL: forbidden '$needle' appeared in $file" >&2
      return 1
    fi
    sleep 0.25
  done
  return 0
}
wait_for "$workdir/a.out" 'mode: '
wait_for "$workdir/b.out" 'mode: '
if grep -Fq 'SIMULATION' "$workdir/a.out" || grep -Fq 'SIMULATION' "$workdir/b.out"; then
  echo 'FAIL: physical run must not be labeled SIMULATION' >&2
  exit 1
fi
wait_for "$workdir/a.out" 'link: connected'
wait_for "$workdir/b.out" 'link: connected'

# Send an operator move, retrying only when the client rejected the input
# locally (link degraded at the moment of typing). A delivery-unknown or a
# rejected peer move is never retried: that is a protocol outcome, not typing.
play_move() {
  local fd="$1" sender_out="$2" square="$3" seq="$4" baseline
  for _ in 1 2 3 4 5 6; do
    baseline=$(grep -cE 'not your turn|occupied|game is already over' "$sender_out" || true)
    printf '%s\n' "$square" >&"$fd"
    for _ in $(seq 1 "$WAIT_TRIES"); do
      if grep -Fxq "seq $seq: delivered" "$sender_out"; then return 0; fi
      if grep -Fxq "seq $seq: delivery unknown" "$sender_out"; then
        echo "FAIL: seq $seq delivery unknown for square $square" >&2
        return 1
      fi
      if (( $(grep -cE 'not your turn|occupied|game is already over' "$sender_out" || true) > baseline )); then
        wait_for "$sender_out" 'link: connected'
        break
      fi
      if ! kill -0 "$A_PID" 2>/dev/null || ! kill -0 "$B_PID" 2>/dev/null; then break; fi
      sleep 0.25
    done
  done
  echo "FAIL: square $square never reached seq $seq delivered" >&2
  return 1
}

# A (X) plays center. B tries the occupied square first: it must be rejected
# locally and never appear as a peer move on A.
# A's start control consumes seq 1; A's moves are seq 2, 3, 4 and B's are 1, 2.
play_move 3 "$workdir/a.out" 5 2
wait_for "$workdir/b.out" 'peer move 5 (ply 0)'
wait_for "$workdir/b.out" ' 4 | X | 6'
printf '5\n' >&4
wait_for "$workdir/b.out" 'square 5 occupied'
wait_absent "$workdir/a.out" 'peer move 5' 3
echo 'PASS: illegal occupied square rejected locally; no peer move emitted'
play_move 4 "$workdir/b.out" 2 1
wait_for "$workdir/a.out" 'peer move 2 (ply 1)'
wait_for "$workdir/a.out" ' 1 | O | 3'
play_move 3 "$workdir/a.out" 1 3
wait_for "$workdir/b.out" 'peer move 1 (ply 2)'
play_move 4 "$workdir/b.out" 8 2
wait_for "$workdir/a.out" 'peer move 8 (ply 3)'
play_move 3 "$workdir/a.out" 9 4
# A winning move is reported as the outcome, not a plain peer-move note.
wait_for "$workdir/a.out" 'X wins'
wait_for "$workdir/b.out" 'X wins'
# Both boards must show the identical final position.
for side in a b; do
  wait_for "$workdir/$side.out" ' X | O | 3'
  wait_for "$workdir/$side.out" ' 7 | O | X'
done
echo "PASS docker $CHANNEL game: legal moves both directions, illegal move rejected, X wins on both boards"
printf '/q\n' >&3; printf '/q\n' >&4
wait "$A_PID" "$B_PID"
