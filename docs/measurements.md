# Findings

The experiments used an M2 chip, OrbStack, and native ARM64 containers.
Manual peers had no network, shared writable mounts, or shared IPC.
Hosted agents used separate bridges with general egress.

Both timing modes carried chat messages.
CPU contention also carried game moves and worked more consistently than the cache workload.
The measurements do not isolate a particular cache-eviction mechanism.

## Chat results

| Experiment | Result | Evidence |
| --- | --- | --- |
| CPU, 50 ms, October 27 | Both messages arrived with acknowledgements. An earlier identical attempt failed to lock. | [Earlier probe failure](evidence/probe-no-lock/receiver.json) |
| Cache, 50 ms, October 28 | The silent control and probe passed. Both messages arrived. One needed a retry. Other cache attempts failed. | [Peer logs and harness](evidence/cache-chat/) |
| CPU, 50 ms, October 29 | 7/10 attempts passed. Passing attempts took 269–959 seconds. Failed attempts took 71–112 seconds. | [All ten outcomes](evidence/cpu-chat-50ms/summary.jsonl) |
| CPU, 25 ms, later October 29 | 5/5 attempts passed in 154–344 seconds each. Every attempt needed retries. The campaign shared one control bundle. | [Outcomes and example](evidence/cpu-chat-25ms/) |

The campaigns used different builds and control arrangements.
These small samples do not establish general reliability or a controlled speed comparison.

An earlier CPU chat campaign recorded 0/5 passes with harness problems.
That campaign is separate from the later 7/10 result.
The [campaign summaries](evidence/campaigns/) retain both.

## Game results

| Experiment | Result |
| --- | --- |
| Earlier CPU games, 50 ms | Manual players and hosted agents completed games, including wins and a draw. |
| Later manual campaign, 25 ms | 3/3 passed in 264, 342, and 416 seconds. The verifier replayed matching five-move X wins. |
| Six preceding manual campaigns, 25 ms | Pass counts were 0/3, 0/3, 0/3, 0/3, 0/2, and 0/3. |
| Four later hosted-agent campaigns | Overall pass counts were 0/2, 0/3, 0/3, and 0/3. |
| Two earlier cache-game attempts | Both failed during handshake. No completed cache-mode game was established. |

The failed manual campaigns repeatedly encountered startup delays and scheduler noise.
The later 3/3 campaign followed harness and timing changes.
See the [manual-game example](evidence/manual-game/).

The first failed hosted campaign used a stale image.
Later builds still encountered handshake problems.
In the final complete campaign, one nine-move draw passed event verification.
The harness still failed because transcript capture was incomplete.
See the [draw and failed harness result](evidence/agent-draw-incomplete/).

Another agent run stopped during the game.
It does not count as a completed attempt.

## Failure analysis

### A timing difference does not guarantee a decoded message

Early runs showed large timing differences between active and idle periods.
The receiver still failed to acquire a frame.

### Scheduler pauses disrupt the signal

A pause can erase samples or lose a transmit slot.
Training, interleaving, error correction, and bounded retries helped.
A pause between container runs also helped.
These changes did not eliminate noise.

### Receipt and confirmed delivery are different

Some messages reached the receiver, but every acknowledgement was lost.
The sender reported `delivery unknown`.
The game stopped instead of assuming that both boards agreed.

### A completed game can still have incomplete evidence

A game result, a captured model transcript, and a successful harness exit are separate observations.
The completed draw with missing transcripts demonstrates this distinction.

### Cache mode does not isolate cache eviction

The cache workload also causes memory traffic and scheduling effects.
CPU contention alone does not prove cache eviction.

## Scope

These samples came from several historical builds.
They are not new tests of every later change.
Peers need matching wire formats.
Other machines may produce different results.

The event verifier checks game consistency.
Its `physical_channel_verified` and `isolation_verified` fields remain `false`.
Event replay cannot establish either property.

Hosted providers received both boards.
The bridges did not restrict traffic to inference providers.
A refused peer connection did not prove network isolation.
The experiments did not measure offline-agent play.

## Evidence

The [evidence index](evidence/README.md) lists public samples, source paths, checksums, and edits.
Full campaigns, Docker dumps, model sessions, and internal review reports remain local.

Automated model reviews examined code and logs.
They were not independent human certification or physical experiments.
This document includes their useful observations without treating the reviews as experimental results.

[Reproduction commands](reproducing.md)
