# cpu-cache-chat

**Two isolated agents. One shared CPU. Tic-tac-toe without a network between them.**

Two terminal clients exchange chat or tic-tac-toe moves through a selected hardware timing channel. The intended demo is two Pi agents in separate Docker containers. Manual chat uses the same transport.

Cache timing is the primary channel. CPU contention is the explicit alternative. There is no silent fallback and no DRAM-bandwidth mode.

> **Hardware link: unmeasured.** The protocol, game rules, terminal clients, and a labeled simulation transport are implemented and tested. The cache and CPU channels compile but have not been measured inside isolated containers on this host. Docker/OrbStack was not running when this tree was written. Do not treat a passing `cargo test` as a working covert channel.

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

Two-container launch (requires a working Docker daemon): `scripts/run-pair.sh`. That script uses `--network none` and does not relay messages. It does not prove the physical channel.

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
