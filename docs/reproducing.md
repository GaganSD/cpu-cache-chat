# Reproducing the experiments

Use a machine you control.
The tests deliberately create CPU or memory contention.
Run one experiment at a time.

## Setup

| Requirement | Version or platform |
| --- | --- |
| Container runtime | OrbStack on an Apple-silicon Mac, or Docker Engine on Linux |
| Scripts | Bash and Python 3 |
| Host-side Rust tests | Rust 1.96.1 |
| Agent tests | Node 22.19 or later |

The Dockerfiles build native ARM64 or x86-64 images.

1. Build the manual image:

   ```sh
   ./docker/build.sh
   ```

2. Check Docker access:

   ```sh
   ./scripts/demo.sh doctor
   ```

The scripts default to Docker guest CPU 0.
If Docker does not allow that CPU, select another guest CPU.
Use `CPUSET` for harnesses or `--cpu` for the launcher.
A guest CPU does not necessarily map to one fixed physical core.

## Chat

Run one CPU-contention test:

```sh
CCC_DEBUG=1 CHANNEL=cpu SLOT_MS=50 CPUSET=0 WAIT_SECONDS=900 \
  EVIDENCE_DIR="$PWD/measurements/raw/cpu-chat" ./scripts/e2e-docker.sh
```

The harness performs these steps:

1. Check that a silent sender produces no decoded frame.
2. Send a probe without supplying its expected payload to the receiver.
3. Compare the decoded probe with the expected payload after the probe exits.
4. Send a chat message in each direction.
5. Check both receipts and acknowledgements.

Repeat with `CHANNEL=cache` to test the cache workload.
A failed probe stops the test.
It does not change the transport.

### Read the result

`link: connected` alone does not mean chat succeeded.
Both messages need receipts and acknowledgements.

A timeout does not mean zero errors.
A JSON `null` error count does not mean zero errors.

Some older probes printed `bit_errors: 0` without decoding a frame.
Read their timeout fields and notes before interpreting that value.

## Chat campaigns

Run five attempts:

```sh
CHANNEL=cpu SLOT_MS=25 CPUSET=0 CCC_LOAD_NOTE="normal desktop use" \
  EVIDENCE_DIR="$PWD/measurements/raw" ./scripts/run-physical-matrix.sh chat 5
```

The campaign runs the silent control and probe once for a fixed configuration.
It saves the results as a control bundle.
Each chat attempt references that bundle.

The configuration includes:

- Image ID.
- Channel.
- Slot duration.
- CPU assignment.
- Load note.

Individual attempts may retransmit.
The summary records durations and successes without retries separately.
Keep failed attempts in the summary.
Do not apply one build's results to another build.

## Manual games

Run one game:

```sh
CHANNEL=cpu SLOT_MS=25 CPUSET=0 \
  EVIDENCE_DIR="$PWD/measurements/raw/manual-game" ./scripts/e2e-docker-game.sh
```

The harness tests legal moves, rejection of an occupied square, and agreement between the final boards.

## Hosted-agent games

1. Build the agent image:

   ```sh
   ./docker/build-agent.sh
   ```

2. Export `OPENCODE_GO_API_KEY` in the shell.

3. Run the game:

   ```sh
   CCC_HOSTED_OPT_IN=1 CHANNEL=cpu SLOT_MS=50 CPUSET=0 \
     EVIDENCE_DIR="$PWD/measurements/raw/agent-game" ./scripts/e2e-docker-agent.sh
   ```

The harness defaults to OpenCode Go.
The [agent guide](../agent/README.md) describes the other provider.

The provider receives board prompts and may charge for requests.
Each peer uses a separate bridge with general egress.
The bridges do not restrict traffic to the provider.
These experiments did not test local-model setup or offline-agent play.

## Game verification

The verifier replays both event files.
It checks legal moves and matching boards.
When requested, it also compares recorded agent choices with accepted moves.

Event replay does not check physical transport or container isolation.
A game can finish while its harness fails because evidence is missing.
Keep both the game result and harness result.

## Record the run

Record these values:

| Category | Values |
| --- | --- |
| Build | Source revision, dirty state, and image ID |
| Environment | Host architecture, CPU assignment, and load |
| Transport | Channel and slot duration |
| Test | Controls, duration, and result |

Image labels record the supplied source revision.
They do not prove which source produced the image.
Never send one peer's logs to the other peer.

## Publish samples

Git ignores raw output under `measurements/`.
Keep that output locally.

Before publishing a sample:

1. Review its contents.
2. Copy the selected files into `docs/evidence/`.
3. Document edits to the files.
4. Preserve the context of both successes and failures.

Do not publish full model sessions or Docker environment dumps by default.
The existing [samples](evidence/README.md) are historical observations, not new runs of the current checkout.

## Software-only checks

Run these commands from the repository root:

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

Simulation sends messages through Unix datagrams.
It tests the software, not the hardware channel.
