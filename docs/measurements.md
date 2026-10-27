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
