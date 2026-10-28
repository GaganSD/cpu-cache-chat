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

Docker Engine on Linux or OrbStack on Apple silicon, Bash, Python 3, and a native image are required for physical demos. Building from source also needs Rust 1.96.1. The launcher is implemented and software-tested; the current wire format's physical release gates are not yet complete:

```sh
./docker/build.sh
./scripts/demo.sh doctor
./scripts/demo.sh chat --channel cpu
# Use the run ID printed by the launcher:
./scripts/demo.sh stop --run-id RUN_ID
```

The launcher prints two `docker attach` commands: run each in its own terminal, type a short personal message such as `hello world`, and reply from the other. Each terminal controls only its own peer. Detach with Ctrl-p Ctrl-q, or stop both using the printed run ID. The launcher runs a bounded receiver-blind probe first; a failed probe stops the launch without switching channels. A pair should show its channel, connection and delivery state; no-lock and unknown delivery are possible. At the explicit 50 ms reference preset, one protocol control frame nominally takes 20.15 seconds; `hello world` plus its ACK takes 49.4 seconds, before token wait, retries, startup, and scheduling delays. Handshakes and games can take minutes or time out. Do not expect interactive-network latency. To try a manual game, use `./scripts/demo.sh game --channel cpu`. Hosted agents additionally require `./docker/build-agent.sh`, `CCC_HOSTED_OPT_IN=1`, and provider credentials. Use `./scripts/demo.sh agents --channel cpu --provider opencode-go` (`OPENCODE_GO_API_KEY`) or `--provider vercel-ai-gateway` (`AI_GATEWAY_API_KEY`). The configured models are DeepSeek V4.1 Flash at low reasoning and Reflection Beam Free, respectively. Provider availability, pricing, and tool reliability are external dependencies. Both boards are disclosed to the provider; separate bridges permit general egress, not only inference. Never put keys in logs or images.

For software-only checks, use `cargo test --workspace --locked` and `MODE=chat ./scripts/e2e-headless-sim.sh`. The latter is not a physical test. If Docker is unavailable, start the runtime; if connection fails, stop the run and inspect its diagnostics rather than assuming success or switching channels silently.

[Architecture](docs/architecture.md) · [Measurements](docs/measurements.md) · [Release checklist](docs/release.md) · [Contributing](CONTRIBUTING.md) · [Security](SECURITY.md)
