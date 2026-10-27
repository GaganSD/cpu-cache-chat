# Channel measurements

Recorded 2026-10-27 on Apple M2, OrbStack 2.2.5, Docker 29.8.2, ARM64.

## Software path

| Test | Isolation | Result |
|---|---|---|
| `MODE=chat ./scripts/e2e-headless-sim.sh` | two host processes, `--sim` | PASS both directions |
| `MODE=game ./scripts/e2e-headless-sim.sh` | two host processes, `--sim` | PASS both directions |
| `cargo test --workspace --release` | n/a | PASS |

Simulation is Unix datagrams. It is not a hardware channel.

## Isolation

`cpu-cache-chat:local` image built. Two containers with `--network none --ipc private --cap-drop ALL --security-opt no-new-privileges --user 65532`. Inspect: `network=none privileged=false`. No docker.sock.

OrbStack Linux machines created: `ccc-vm-a` and `ccc-vm-b` (debian bookworm, 1G, arm64). Alpine was tried first; the glibc binary does not run on musl.

## Physical timing (not a working link)

Shared-vCPU bench (`bench --channel cpu --slot-ms 5`, both pinned to cpuset 0 or two VMs):

| Pair | n | min ns | max ns | max/min |
|---|---|---|---|---|
| Docker ccc-bench A/B, cpu | 600 | 11667 | 93251 | 8.0 |
| Docker ccc-bench A/B, cache | 598 | 123960 | 6327818 | 51.0 |
| VMs ccc-vm-a/b, cpu | 600 | 11958 | 881176 | 73.7 |

There is timing contrast. That is not a decoded frame.

Handshake (`chat --plain --channel cpu --slot-ms 5`) in Docker and on the host did **not** reach `link: connected`. Debug hunts showed preamble Hamming distance ~14–18 (random). Carrier energy is visible; bit lock is not.

| Date | Host | Isolation | Mode | Result |
|---|---|---|---|---|
| 2026-10-27 | M2 / OrbStack Docker | two containers, cpuset 0 | cpu | handshake failed |
| 2026-10-27 | M2 / OrbStack Docker | two containers, cpuset 0 | cache | bench contrast only |
| 2026-10-27 | M2 host processes | none | cpu | handshake failed |
| 2026-10-27 | OrbStack debian VMs | two machines | cpu | bench contrast; no chat lock |

Do not treat bench ratio as a working chat or cache-eviction proof. Cache contrast includes scheduling and memory traffic.

## 2026-10-27 retest: experimental PHY

On the same Mac/OrbStack Docker host, the image was rebuilt with the iterative, quarter-slot hunt and interval averaging. Two separate `--network none`, private IPC, read-only containers were pinned to CPU 0; neither was given the other's logs. Each sender chose `5a 7f a4 c9`; the receiver had no expected bytes. Comparison happened only after both exited. One receive attempt per row, not a reliability estimate:

| Command (`CCC_DEBUG=1 CPUSET=0 BITS=32 CHANNEL=... SLOT_MS=... ./scripts/probe-pair.sh`) | Result | Lowest printed preamble Hamming distance |
|---|---|---:|
| cache, 10 ms | timeout; no frame; BER unavailable | 10 |
| cpu, 10 ms | timeout; no frame; BER unavailable | 15 |
| cache, 20 ms | timeout; no frame; BER unavailable | 14 |
| cpu, 20 ms | timeout; no frame; BER unavailable | 15 |
| cache, 50 ms | timeout; no frame; BER unavailable | 13 |
| cpu, 50 ms | timeout; no frame; BER unavailable | 15 |

Debug output is sampled, so the minimum printed distance is not the minimum over every candidate. Each receiver ended with `timeouts: 1`, `usable: false`, and `receive failed: timeout; BER unavailable`; the script reported `FAIL: ... probe comparison`. The JSON `bit_errors: 0` field is not a measurement when no frame is decoded. No working slot was found. Both channels are **unusable in this test**; a timing contrast or simulation success does not establish a hardware channel.

A cache receiver run alone with `docker run --rm --init --network none --ipc private --read-only --tmpfs /tmp:rw,nosuid,size=64m --security-opt no-new-privileges --cap-drop ALL --user 65532:65532 --memory 256m --cpuset-cpus 0 cpu-cache-chat:local probe --role b --channel cache --slot-ms 10 --silent --silent-seconds 5` reported `false decoded frame=false; result=Err(Timeout)`. This is a single silent control, not proof of a reliable false-positive rate.

The receiver still uses an active-high fixed threshold, may lock off-center, and can drift or skip slots under scheduling delays. Its tests do not establish physical decoding. In addition, the current fixed four-second handshake receive slice is too short for a control frame at the larger slot lengths. No physical chat or game result is claimed.

`CCC_DEBUG=1 CHANNEL=cpu SLOT_MS=10 ./scripts/e2e-docker.sh` also reported `false decoded frame=false; result=Err(Timeout)` for the silent control, then inspected both probe containers (`network=none privileged=false pid= ipc=private`, no mounts). Its receiver timed out and the script exited 1 with `FAIL: receiver-blind probe did not decode all expected bits; BER unavailable if no frame decoded`. Chat was not attempted because the gate failed. The physical game and agent checklist was not run after this stop condition.

`cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `./docker/build.sh`, and both `MODE=chat` and `MODE=game ./scripts/e2e-headless-sim.sh` passed. The simulation checks include both clients' software-only label, both directions, peer board rows, and escaped terminal control bytes. `bash scripts/tests/inspect-isolation.sh` accepted isolated containers and rejected bridge networking, privileged mode, runtime-directory and Docker socket binds, and overlapping writable binds. After the runs, `docker ps -a --filter name=ccc-` returned no containers, and OrbStack displayed “No Containers”.

## 2026-10-27 continuation: CPU contention progress

Two receiver-blind CPU probes on isolated containers pinned to CPU 0 at 50 ms decoded 4 and 8 bytes with CRC and post-run comparisons of 0/32 and 0/64 bit errors, respectively. These were the pre-FEC format; the raw output is in [measurements/2026-10-27-physical](../measurements/2026-10-27-physical/). They do not establish reliability or prove cache eviction.

With interleaved SECDED and CRC, `CCC_DEBUG=1 CHANNEL=cpu SLOT_MS=50 WAIT_SECONDS=480 EVIDENCE_DIR=/tmp/ccc-evidence ./scripts/e2e-docker.sh` passed its silent control and receiver-blind probe, and both chat peers reported `link: connected`. In the retry run, B received `hello-from-a` and A reported sequence 1 delivered after a retry. The script then interrupted B's transmission because debug stderr and stdout interleaved; the harness has since been changed to keep the streams separate. In a later run with separate streams, B received the message once and suppressed a duplicate, but A rejected acknowledgements and the script timed out before a confirmed delivery. Sender diagnostics recorded whole-slot overruns, including 309 and 414 ms beyond a 50 ms slot. Neither run established B→A delivery. Full captured test logs are preserved in the measurement directory.

A soft-decision FEC decoder was subsequently added and passed synthetic tests, but **has not yet been tested on the physical channel**. The CPU channel is promising but bidirectional hardware chat and game remain unverified; the cache trial decoded payload bytes but failed CRC. Simulation remains a software-only path.

## 2026-10-27 continuation: bidirectional CPU chat verified; cache partial; physical game completed

The image was rebuilt at the checked-out commit and Docker layer caching produced the identical digest (`sha256:6a96704b888d...`), confirming the tested image matches the source. All runs used `CCC_DEBUG=1 SLOT_MS=50 WAIT_SECONDS=480 CPUSET=0`, two `--network none` containers with the documented isolation flags; evidence is host-side only. The channel remains stochastic: identical commands fail or pass depending on host scheduler noise, so each run below is reported individually.

**CPU contention — bidirectional chat PASS** (`CHANNEL=cpu ./scripts/e2e-docker.sh`, evidence `cpu-chat-bidirectional/`): silent-sender control passed (`false decoded frame=false; result=Err(Timeout)`), the receiver-blind probe decoded `5a 7f a4 c9` with 0 bit errors, both peers reached `link: connected`, `hello-from-a` and `hello-from-b` each progressed queued → transmitting → awaiting ack → delivered, and each peer printed the other's message. This is the first verified bidirectional hardware chat. An identical earlier attempt the same day failed at the probe with no preamble lock (weak 18.1 µs active vs 12.3 µs idle contrast; the passing run trained near 23 µs vs 12 µs); see `cpu-probe-no-lock/`.

**Cache timing — partial** (`CHANNEL=cache ./scripts/e2e-docker.sh`, evidence `cache-chat-partial/`): silent control passed and the receiver-blind probe decoded the payload with 0 bit errors and a passing CRC — the first physically decoded cache-mode frame. A→B `hello-from-a` was delivered and acknowledged. B→A the payload did arrive (A printed `peer: hello-from-b`) but A's acknowledgements repeatedly failed (`missed physical slot`, CRC mismatches), so B exhaustedly reported `delivery unknown` after 6 attempts; the link degraded. The soft-decision FEC decoder corrected 86 bits across the two clients in this run — its first physical use. Two subsequent cache runs failed at the receiver-blind probe. Cache mode is therefore **not** bidirectionally verified; do not conflate it with the CPU result.

**CPU contention — game completed physically** (`CHANNEL=cpu ./scripts/e2e-docker-game.sh`, evidence `cpu-game-x-wins/`): with the new game harness, moves 5 (X), 2 (O), 1 (X), 8 (O), 9 (X) were each transmitted and acknowledged over the physical channel; an occupied-square input was rejected locally (`square 5 occupied`) and emitted no peer move; both clients displayed the identical final board (`X|O|3 / 4|X|6 / 7|O|X`) and `X wins`. The harness itself reported FAIL on this run only because it waited for a `peer move 9` note that a winning move never prints and used sequence numbers off by one from the start-control frame; both assertions were corrected in the script. A full PASS rerun of the corrected harness was paused mid-game (after the illegal-move PASS) and has not yet completed. A flaky mid-game link degradation also showed the intended honest behavior: an input typed while the link was degraded was rejected locally (`not your turn or link is not connected`) and no move was committed.

Remaining unverified: a completed harness-PASS game run, cache bidirectional chat or game, the Pi agent path over the physical transport, and any reliability statistics (each result above is a single run).
