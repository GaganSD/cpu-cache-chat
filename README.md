# cpu-cache-chat

Two sandboxed programs share a CPU and attempt to exchange chat or tic-tac-toe moves using timing rather than a peer network. This experiment was inspired by accounts of agent communication through shared hardware; no equivalence to any specific research is asserted here.

## Status and scope

| Configuration | Evidence and limits |
| --- | --- |
| Simulation | Unix datagrams, for software testing only; not isolation or physical-channel evidence. |
| CPU contention / cache timing | Historical single-run results on Apple M2 and OrbStack exist; failures also occurred. The newer wire format needs fresh physical validation. |
| Hosted agents | Historical hosted runs used external inference and separate bridge networks, not offline operation. Egress was not restricted to inference destinations. |
| Offline agents, other Macs, Linux | Physical release gates blocked pending measurements; do not infer support from CI. |

See [measurements](docs/measurements.md). Containers share a Linux kernel; they are not a security boundary against all covert channels. Cache timing does not establish a particular eviction mechanism. The link can fail or report unknown delivery; never treat it as reliable or secure.

## Prerequisites and quickstart

Docker Engine on Linux or OrbStack on Apple silicon, Bash, and a native image are required for physical demos. The following launcher interface is **planned** and must be verified against the checkout before use; physical gates are not yet complete:

```sh
./docker/build.sh
./scripts/demo.sh doctor
./scripts/demo.sh chat --channel cpu
# Use the run ID printed by the launcher:
./scripts/demo.sh stop --run-id RUN_ID
```

A pair should show its channel, connection and delivery state; no-lock and unknown delivery are possible. At 50 ms per slot, even an empty physical frame can take several seconds; handshakes, retries and games can take minutes or time out. Do not expect interactive-network latency. To try a manual game, use `./scripts/demo.sh game --channel cpu`. Hosted agents are opt-in with `./scripts/demo.sh agents --channel cpu --provider opencode-go`, require credentials and incur external inference costs. Never put keys in logs or images.

For software-only checks, use `cargo test --workspace --locked` and `MODE=chat ./scripts/e2e-headless-sim.sh`. The latter is not a physical test. If Docker is unavailable, start the runtime; if connection fails, stop the run and inspect its diagnostics rather than assuming success or switching channels silently.

[Architecture](docs/architecture.md) · [Measurements](docs/measurements.md) · [Release checklist](docs/release.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)
