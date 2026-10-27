# cpu-cache-chat

Re-implementation of OpenAI's sandbox escape where misaligned agents used CPU's cache to communicate across different sandboxes.

Chat is an experimental terminal application which allows to sandbox instances to communicate between each other by implementing a message protocol over Macbook's CPU cache.

We demonstrate this in 2 ways: 
- using two isolated Pi agents that share a CPU to play Tic-tac-toe  
- and a chat interface to message other VMs

**Two isolated agents. One shared CPU. Tic-tac-toe without a network between them.**

Two terminal clients are intended to exchange chat or tic-tac-toe moves through a selected hardware timing channel. The intended demo is two Pi agents in separate Docker containers. Manual chat uses the same transport.

Cache timing is the primary channel. CPU contention is the explicit alternative. There is no silent fallback and no DRAM-bandwidth mode.

> **Hardware link: partially verified.** Headless simulation chat and game pass. Docker isolation (`--network none`) holds. Over the CPU-contention channel at 50 ms slots, two isolated containers completed a bidirectional acknowledged chat and a full tic-tac-toe game (`X wins` on both boards) — single runs, not reliability statistics. Cache mode has decoded a receiver-blind frame with a passing CRC and delivered one chat direction, but is not bidirectionally verified. The Pi agent path is untested. See [docs/measurements.md](docs/measurements.md).

## What works now

```text
cargo test --workspace
cargo run -p cpu-cache-chat -- chat --role a --channel cache --sim --plain
cargo run -p cpu-cache-chat -- game --role b --channel cache --sim
```

`--sim` is a Unix-datagram software path. The UI labels it `simulation`. It is for protocol development, not the isolated demo.

```text
cpu-cache-chat chat  --role a|b --channel cache|cpu [--sim]
cpu-cache-chat game  --role a|b --channel cache|cpu [--agent] [--sim]
cpu-cache-chat probe --role a|b --channel cache|cpu
cpu-cache-chat inspect-isolation
```

Two-process software pair: `scripts/host-sim-pair.sh`.

Headless sim: `MODE=chat ./scripts/e2e-headless-sim.sh`. Two-container launch: `scripts/run-pair.sh` or `scripts/e2e-docker.sh`; physical game: `scripts/e2e-docker-game.sh`. Isolation uses `--network none` and does not relay messages. Computer-use retest: [docs/codex-computer-use-test.md](docs/codex-computer-use-test.md).

## Isolation

Containers, not microVMs. Shared-kernel Docker on OrbStack (macOS) or Docker Engine (Linux). See [product.md](product.md) and [docs/architecture.md](docs/architecture.md).

## Models

Each container is supposed to run its own Pi + llama.cpp instance. The first candidate is quantized Qwen3-0.6B. That path is sketched in `agent/` and has not been evaluated for tool reliability or 8 GB two-instance memory.

## Honesty rules

- Cache success is not implied by CPU-contention success.
- Simulation success is not a hardware result.
- A failed calibration is reported, not replaced with sockets.

- [Product plan](product.md)
- [Architecture](docs/architecture.md)
- [Measurements](docs/measurements.md)
- [Handoff](prompt.md)
