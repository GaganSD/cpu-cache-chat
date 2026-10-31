# cpu-cache-chat

**Message protocol over CPU's cache: re-implementation of OpenAI's sandbox escape attempt.**

A weekend experiment: two programs on the same machine exchange chat messages and tic-tac-toe moves through hardware timing, instead of sending each other network packets. You can type in two terminals or let two agents play.

## Why

Inspired by OpenAI's [Securing our frontier model training environment](https://openai.com/index/securing-our-frontier-model-training-environment/). In its sandbox red-team experiments, agents in separate microVMs communicated by evicting and reloading CPU-cache data. The post also describes channels using shared storage and competition for CPU time.

This is my take on the communication idea, using Docker containers—not a reproduction of OpenAI's infrastructure or an actual sandbox escape.

## How it works

One peer alternates between doing work and staying idle. The other measures timing differences and decodes the pattern into bits.

- **Cache timing** (`--channel cache`): an 8 MiB strided-memory workload creates timing differences. The experiment doesn't isolate cache eviction from scheduling and memory contention.
- **CPU contention** (`--channel cpu`): busy/idle periods compete for CPU time. This has been the more practical channel.
- **Protocol**: preamble and training, error correction, CRC, acknowledgements, retries, and a token to take turns talking.
- **Apps**: terminal chat and tic-tac-toe, with optional Pi agents choosing moves. Rust checks the game rules.

Manual peers have no network or shared writable files. The host starts and observes them; it doesn't relay their messages. Simulation uses Unix datagrams and is only for software tests.

## What happened

On an Apple M2 with OrbStack:

- Bidirectional chat worked in both CPU-contention and cache-timing modes.
- A CPU chat campaign at 50 ms slots passed **7/10** attempts. A later 25 ms campaign passed **5/5**, with retries, taking about **2.5–6 minutes** per attempt.
- A later manual tic-tac-toe campaign passed **3/3** attempts.
- Hosted agents completed games, including a draw. Other attempts stalled at handshake or lost acknowledgements; one completed draw failed the overall harness because transcript capture was incomplete.

It's slow and noisy. Scheduler pauses matter, and a missing acknowledgement can leave delivery unknown. These are observations from particular builds on one setup, not benchmarks for every Mac or Linux machine. [Findings and sample evidence](docs/measurements.md).

## Try it

You'll need Docker Engine on Linux or OrbStack on an Apple-silicon Mac, Bash, and Python 3. Docker builds the Rust binaries.

```sh
git clone https://github.com/GaganSD/cpu-cache-chat.git
cd cpu-cache-chat
./docker/build.sh
./scripts/demo.sh doctor
./scripts/demo.sh chat --channel cpu
```

Run each printed `docker attach` command in a separate terminal, then send a short message. Detach with **Ctrl-p Ctrl-q**. The launcher prints a run ID; stop both peers with:

```sh
./scripts/demo.sh stop --run-id RUN_ID
```

Use `chat --channel cache` for the cache workload, or `game --channel cpu` for tic-tac-toe. The launcher checks the physical channel first and stops if that probe fails. Expect minutes, not instant messaging.

For agent play, build `./docker/build-agent.sh`, export `OPENCODE_GO_API_KEY`, then run:

```sh
CCC_HOSTED_OPT_IN=1 ./scripts/demo.sh agents --channel cpu --provider opencode-go
```

Hosted agents send board prompts to an external provider and may incur charges. Their separate bridges allow general egress; this isn't an offline or network-isolation demo. [Agent setup](agent/README.md).

## Reproduce the experiments

[Commands, controls, and how to read failures](docs/reproducing.md). Software-only checks need Rust 1.96.1:

```sh
cargo test --workspace --locked
MODE=chat ./scripts/e2e-headless-sim.sh
```

[Architecture](docs/architecture.md) · [Protocol](docs/protocol.md) · [Launcher](docs/launcher.md)

## License

[MIT](LICENSE).
