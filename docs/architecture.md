# Implementation architecture

This is the locked contract for `cpu-cache-chat`. Follow it. Do not invent sockets, shared files, host relays, or silent channel fallback.

## Layout

| Path | Responsibility |
|---|---|
| `crates/ccc-core` | Shared types, errors, budgets, config |
| `crates/ccc-protocol` | Frame codec, session, stop-and-wait reliability |
| `crates/ccc-game` | Tic-tac-toe rules only |
| `crates/ccc-channel` | Physical transports: cache timing, CPU contention, labeled simulation |
| `crates/ccc-app` | Chat and game sessions over the protocol |
| `crates/ccc-cli` | `cpu-cache-chat` binary: clap + ratatui |
| `tests/` | Workspace integration tests (keep tests out of `src/`) |
| `crates/*/tests/` | Crate-level integration tests |
| `agent/` | Narrow Pi SDK player (container-local) |
| `docker/` | One packaging path for OrbStack and Docker Engine |
| `scripts/` | Host launch and isolation inspection |
| `measurements/` | Recorded probe results; never invent success |

## Isolation

Two Docker containers. Separate network, PID, IPC, and mount namespaces. No peer/external network (`--network none`). No privileged mode, host namespaces, Docker socket, or shared writable mounts. Immutable image layers may be shared. Loopback inside a container is allowed for that player's model server only.

The host starts/stops containers and attaches terminals. It must not forward payloads, boards, acks, or turns.

## Channels

Select explicitly with `--channel cache` or `--channel cpu`. Never switch silently.

- **cache:** sender disturbs cache residency (large-stride occupancy / eviction walk). Receiver times its own probe array. Generic CPU slowdown is not sufficient evidence.
- **cpu:** sender is register-only busy or idle. Receiver times a compute loop with a tiny working set.
- **sim:** in-process or localhost pipe for software tests. Must display `mode: simulation` and must not be used as the isolated demo.

Both physical modes share slotting:

1. Calibrate slot length and a 0/1 threshold. Record silent-sender control samples.
2. Each frame: 32-bit preamble `0xAAAAAAAA`, 16-bit big-endian length, payload, CRC32 of payload.
3. One bit per slot. Bit `1` = activity, bit `0` = idle.
4. Hunt preamble every frame. Bound waits. Surface timeout.

ARM64 and x86_64 must both compile. Use `Instant` plus optional arch counters (`cntvct_el0`, `rdtsc`) behind `cfg`. Do not require `clflush` on ARM.

## Protocol

Little-endian protocol frames inside the channel payload:

```
magic[2]=C3C4 | version=1 | ftype | flags | session_le[4] | seq_le[2]
| ack_seq_le[2] | len_le[2] | payload[len] | crc32_le[4]
```

CRC32 (ISO/HDLC, `crc32fast`) covers every byte before the CRC. `len <= 256`.

Frame types: `Sync=0 SyncAck=1 Data=2 Ack=3 Nack=4 Heartbeat=5 Resync=6 Close=7`.

Reliability: half-duplex stop-and-wait, duplicate suppression, bounded retries, stale-session rejection, explicit unknown delivery when an ack is missing after retries.

Role A initiates `Sync`. Role B answers `SyncAck`. Connected requires that exchange over the selected channel.

Application payloads (inside `Data`):

| tag | body |
|---|---|
| `0x01` chat | `id_le[2] text_utf8` |
| `0x02` move | `ply square mark` (`square` 1-9, `mark` 1=X 2=O) |
| `0x03` game-ctrl | `kind` (`1` start, `2` abort) |

Terminal control bytes in chat text are data. Render them escaped. Never write raw ESC/CSI from a payload to the tty.

## Game

Squares 1-9, row-major. X (role A) starts. Models choose moves. `ccc-game` validates turns, occupancy, wins, draws. No minimax or scripted fallback.

## CLI

```
cpu-cache-chat chat  --role a|b --channel cache|cpu [--sim]
cpu-cache-chat game  --role a|b --channel cache|cpu [--agent] [--sim]
cpu-cache-chat probe --role a|b --channel cache|cpu [--trials N] [--bits N]
cpu-cache-chat inspect-isolation
```

`--sim` is software-only and must be labeled. Default UI is ratatui; `--plain` prints lines.

Visible state: connecting / connected / transmitting / awaiting ack / delivered / failed / unknown. "Connected" means a peer handshake, not "container running".

## Agent

Pi SDK in `agent/`. Only tools: `get_board`, `submit_move`. No bash, web, or file tools. Talk to llama.cpp on `127.0.0.1` inside the same container. Rust validates every move.

## Honesty

Do not claim a cache channel because CPU contention works. Do not claim microVMs. Do not record fabricated measurements. A well-supported negative result is acceptable.
