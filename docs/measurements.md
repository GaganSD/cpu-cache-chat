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
