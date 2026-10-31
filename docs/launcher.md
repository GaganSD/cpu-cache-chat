# Demo launcher

The launcher creates two peer containers and prints their attach commands.
It also stops the run and collects logs.
Use it for terminal chat, manual games, or hosted-agent games.

## Requirements

- Docker Engine on Linux, or Docker Desktop or OrbStack on an Apple-silicon Mac.
- Python 3.
- A Docker guest CPU that both containers can use.
- A local image built for the selected mode.

Build the manual image with `./docker/build.sh`.
Build the hosted-agent image with `./docker/build-agent.sh`.

## Commands

Run these commands from the repository root:

```sh
./scripts/demo.sh doctor
./scripts/demo.sh chat --channel cpu
./scripts/demo.sh game --channel cache --cpu 0
```

For each run:

1. Open two terminals.
2. Run one printed `docker attach` command in each terminal.
3. Enter messages or moves.
4. Press **Ctrl-p Ctrl-q** to detach without stopping the run.

Stop both peers with the printed run ID:

```sh
./scripts/demo.sh stop --run-id RUN_ID
```

The containers continue until the run stops.

## Images, CPUs, and timing

`--image` selects a local image.
The launcher resolves its immutable image ID before it creates containers.

`--cpu` selects a Docker guest CPU.
The launcher checks the index against the guest's CPU count.
A temporary container then checks whether Docker accepts that CPU assignment.
The index does not identify a physical host core.

Every mode uses the `reference-50ms` preset.
A 19-byte control frame takes about 20 seconds before retries.
Games and retries can take minutes.
The launcher does not silently change channels.

## Preflight checks

`doctor` checks Docker access and reports the guest CPU count.
It does not check images, credentials, or physical communication.

By default, the launcher runs a 32-bit physical probe before it starts the peers.
The receiver does not receive the expected payload as an argument.
The probe has a 300-second deadline.
A failed probe stops the launch.

`--skip-preflight` skips that probe.
Use it only for tests or deliberate troubleshooting.
A passing probe records one observation, not a reliability estimate.

## Hosted agents

Export the provider key before the launch.
Then select the provider:

```sh
CCC_HOSTED_OPT_IN=1 ./scripts/demo.sh agents --channel cpu --provider opencode-go
CCC_HOSTED_OPT_IN=1 ./scripts/demo.sh agents --channel cpu --provider vercel-ai-gateway
```

| Provider | Key variable | Model |
| --- | --- | --- |
| `opencode-go` | `OPENCODE_GO_API_KEY` | `deepseek-v4.1-flash`, low reasoning |
| `vercel-ai-gateway` | `AI_GATEWAY_API_KEY` | `reflection/beam-free` |

The gateway also accepts `VERCEL_AI_GATEWAY_API_KEY` as an alternative key variable.
Board prompts leave the machine.
Provider charges may apply.
The launcher does not install local models or support offline-agent launches.

Manual peers have no network.
Hosted peers use separate bridges with general egress.
These bridges do not restrict traffic to model providers.

For hosted launches, the launcher tests TCP paths with temporary listeners.
It requires successful connections within each bridge and denied or timed-out connections between bridges.
It also requires disabled IPv6.
These checks cover the tested listener IPs and TCP port, not every possible network path.

The launcher passes credentials by environment variable name.
It does not print credentials or store them in manifests.
Docker stores the container environment, which privileged Docker users can inspect.

## Container settings

Each container uses:

- A non-root user and read-only root filesystem.
- Private IPC and cgroup namespaces.
- No extra capabilities.
- The `no-new-privileges` setting.
- Memory, PID, and CPU limits.

These settings do not prove that hardware communication works or that every other communication path is blocked.

## Run files

Each run has a random ownership label.
The launcher uses that label to select resources during stop and failure cleanup.

The default storage directory is `~/.cpu-cache-chat/runs`.
Set `CCC_RUN_DIR` to use another directory.
Each run keeps a private manifest and an artifact directory.

The manifest records:

| Category | Recorded values |
| --- | --- |
| Lifecycle | Status, start time, and end time |
| Configuration | Mode, channel, slot, CPU, image ID, and provider |
| Checks | Probe result and hosted TCP-path results |
| Evidence | Logs, events, transcripts, and observed peer exits |
| Game result | `game_verdict` |
| Warnings | Incomplete evidence and review-before-sharing warning |
| Image details | Source labels, revision labels, and bounded `--version` output |

A probe result is `passed`, `skipped`, or `pending`.
Image labels do not prove which source produced an image.
The launcher does not copy the container's `Env` fields into the manifest.

The launcher retains manifests after stop and failure.
A repeated stop reports the saved state without removing unrelated resources.
The launcher rejects a mismatched run ID or ownership label.

Keep the manifest if cleanup fails.
Repeat the stop command to retry cleanup.
Without the manifest, the launcher cannot stop that run.

## Stop and evidence capture

Game peers use `--plain --events-stdout --events-path /tmp/game.events.jsonl`.
They write JSONL events to tmpfs and copy those events to stdout.
Chat peers use `--plain` without game verification.

The stop sequence is:

1. Read hosted transcripts while the containers still run.
2. Send `SIGINT` to the owned peers.
3. Attempt to read complete event files during the five-second exit window.
4. Wait up to five seconds for each peer's exit.
5. Force-remove a peer that does not exit.
6. Collect Docker logs after exit.
7. Select complete event data from the file capture or stdout copy.
8. Run the game verifier.
9. Remove the run's remaining containers and networks.

The launcher reads container files with `docker exec cat`.
It cannot read tmpfs after the container exits.
If complete file capture fails, it uses the stdout copy.

For hosted runs, `CCC_AGENT_DEBUG=1` enables transcripts at `/tmp/agent-transcript.log`.
A transcript at the 16 MiB limit counts as truncated.

TTY containers combine stdout and stderr.
The launcher still captures each available log stream with an 8 MiB limit and a 60-second deadline.
It terminates the capture process when either limit expires.
Truncated logs mark the run incomplete.

## Event checks

Saved event files contain only JSONL bytes.
They do not contain warning headers.
Warnings appear in the manifest and text artifacts.

The launcher extracts stdout events using the exact `CCC_EVENT ` prefix.
It saves them as `events/*.stdout.jsonl`.
A complete file capture takes precedence over the stdout copy.
If both exist but differ, the run counts as incomplete.

An event stream must contain:

1. A versioned session record.
2. Exactly one final-board record.
3. A terminal `stopped` record with `successful=true` and `evidence_complete=true`.

The full [event verifier](events.md) then checks game rules.
Docker log rotation can remove stdout events.
The launcher therefore also attempts direct file capture.
Docker keeps at most two diagnostic log files of 4 MiB each.

## Game verification

The launcher runs `/usr/local/bin/ccc-verify-game` from the selected image after the peers stop.
Hosted runs add `--require-agents`.

The verifier runs in a temporary observer container with:

- No network and a read-only root filesystem.
- The common resource and privilege limits.
- The run's ownership label.
- Read-only copies of both event files.
- A 30-second deadline.

The copied files use mode `644` inside a private host directory with mode `700`.
The observer does not access peer containers or relay live messages.

If the observer cannot run, the launcher can use a host `ccc-verify-game` binary.
Without a successful verifier result, the game evidence counts as incomplete.
The manifest stores the result as `game_verdict`.

Missing events, malformed events, failed exits, unknown exits, forced removals, and missing or failed verdicts all count as incomplete.
The launcher records the reasons instead of creating replacement evidence.

## Storage limits

| Item | Limit |
| --- | --- |
| Agent transcript | 16 MiB |
| Log capture | 8 MiB per stream |
| Artifacts per run | 64 MiB |
| Retained runs | 8 |
| Total artifact budget | 512 MiB |

Text artifacts can truncate at the run limit.
JSONL artifacts must fit completely.
The launcher refuses a launch that exceeds its retention budget.
It does not delete unknown files.
Remove retained runs manually when necessary.

Text artifacts carry a **REVIEW BEFORE SHARING** warning.
Redaction covers current credential values and common bearer-token or API-key patterns.
It can miss secrets.
Review artifacts before publishing them.
