# Reproducing the experiments

Use a machine you control. These tests deliberately generate CPU or memory contention; run one experiment at a time.

## Setup

- Apple-silicon Mac with OrbStack, or Linux with Docker Engine.
- Bash and Python 3. The Dockerfiles build native ARM64 or x86-64 images.
- For host-side software tests: Rust 1.96.1; Node 22.19+ for agent tests.

```sh
./docker/build.sh
./scripts/demo.sh doctor
```

The scripts default to Docker guest CPU 0. If it isn't available, set `CPUSET` for harnesses or pass `--cpu` to the launcher. A guest vCPU isn't necessarily a pinned physical host core.

## One chat run

```sh
CCC_DEBUG=1 CHANNEL=cpu SLOT_MS=50 CPUSET=0 WAIT_SECONDS=900 \
  EVIDENCE_DIR="$PWD/measurements/raw/cpu-chat" ./scripts/e2e-docker.sh
```

Repeat with `CHANNEL=cache` to try the cache workload. The harness runs a silent-sender control, a probe whose payload is not supplied to the receiver, then sends a message in each direction. It compares payloads only afterward. A probe failure stops the chat test; it doesn't switch transports.

A successful run needs both peer receipts and acknowledgements, not just `link: connected`. A timeout or a JSON `null` error count isn't a zero-error result. Older probe output sometimes printed `bit_errors: 0` even when nothing decoded; its timeout and notes must be read too.

## Campaigns and games

```sh
# Five chat attempts, keeping every result.
CHANNEL=cpu SLOT_MS=25 CPUSET=0 CCC_LOAD_NOTE="normal desktop use" \
  EVIDENCE_DIR="$PWD/measurements/raw" ./scripts/run-physical-matrix.sh chat 5

# One manual game, including an occupied-square rejection and board comparison.
CHANNEL=cpu SLOT_MS=25 CPUSET=0 \
  EVIDENCE_DIR="$PWD/measurements/raw/manual-game" ./scripts/e2e-docker-game.sh
```

A chat campaign qualifies silent/probe controls once per image/channel/slot/cpuset/load-note configuration, then references that bundle. Individual attempts may retransmit. Campaign durations and first-attempt successes are recorded separately. Don't discard failed attempts or apply one build's results to a later build.

For hosted agents, build `./docker/build-agent.sh`, export `OPENCODE_GO_API_KEY`, then:

```sh
CCC_HOSTED_OPT_IN=1 CHANNEL=cpu SLOT_MS=50 CPUSET=0 \
  EVIDENCE_DIR="$PWD/measurements/raw/agent-game" ./scripts/e2e-docker-agent.sh
```

The agent harness defaults to OpenCode Go; [agent setup](../agent/README.md) covers the other provider. External inference sees the board prompts and may cost money. Each peer gets its own egress-enabled bridge, not a provider-only firewall. Local/offline model provisioning wasn't exercised by these experiments.

The game verifier replays both peers' event histories and checks board agreement and, when requested, matching agent choices. It doesn't independently verify physical transport or isolation. A game can finish while the harness still fails because evidence is missing; keep both facts.

## Record and share

Record the source revision, dirty state, image ID, host/architecture, channel, slot, cpuset, load, controls, duration, and result. Build scripts label images with a source revision; labels alone aren't a build attestation. Never feed one peer's logs into the other.

Raw output under `measurements/` is ignored. Keep it locally; copy only reviewed examples into `docs/evidence/`, documenting edits and preserving success and failure context. Don't publish full model sessions or Docker environment dumps by default.

The checked-in [examples](evidence/README.md) are historical observations, not new runs of the current checkout.

## Software-only checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --release --locked -p cpu-cache-chat -p ccc-game
MODE=chat ./scripts/e2e-headless-sim.sh
MODE=game ./scripts/e2e-headless-sim.sh
npm ci --prefix agent
npm test --prefix agent
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
```

Simulation tests the software over Unix datagrams; it isn't a hardware communication experiment.
