# Sample evidence

These files contain captured output from experiments on an M2 chip using OrbStack.
They are not example output or new runs of the current checkout.

## Samples

| Directory | Contents |
| --- | --- |
| [cache-chat](cache-chat/) | One passing 50 ms cache chat. Includes the silent control, probe, peer logs, configuration, harness result, and a retry. |
| [probe-no-lock](probe-no-lock/) | An earlier CPU receiver timeout. Its zero error field is not a measured bit error rate. |
| [cpu-chat-50ms](cpu-chat-50ms/) | All ten outcomes, including seven passes. Includes one failed receiver probe. |
| [cpu-chat-25ms](cpu-chat-25ms/) | All five passing outcomes, including retries. Includes one peer pair and its control-bundle reference, but not the full bundle. |
| [manual-game](manual-game/) | All three outcomes. Includes one five-move game's events, configuration, harness result, and verifier result. |
| [agent-draw-incomplete](agent-draw-incomplete/) | Three overall failures. Includes the completed nine-move draw and its failed transcript capture. |
| [campaigns](campaigns/) | Every retained campaign summary, including partial campaigns, harness failures, and stale-image tests. Counts include only completed attempts with summary rows. |

## Source records

[manifest.json](manifest.json) records these values for each sample:

- Original relative source path.
- Original SHA-256.
- Published SHA-256.
- Changes to the contents.

The source revision identifies the snapshot before history cleanup.
It does not link to an accessible historical commit.

## Edits

- Absolute checkout paths use the placeholder `<checkout>`.
- Some output filenames use `.txt` or `.json` for readability.
- The agent harness excerpt ends before the diagnostic peer-log tails.
- That excerpt retains success messages, warnings, the verifier result, and the overall failure.
- Other sampled contents remain unchanged.
- Session IDs, image IDs, revisions, error fields, and failures remain in the samples.

Full campaigns, Docker dumps, model transcripts, and automated reviews remain local.
These samples are not a complete archive or an independent audit.

A historical harness message records what the harness printed.
It does not add proof of isolation or physical transport.
The verifier's `ok: true` means that the event histories agree.

Read the [findings](../measurements.md) with these files.

## Replay games

Run these commands from the repository root:

```sh
cargo build --locked --release -p ccc-game
./target/release/ccc-verify-game docs/evidence/manual-game/a.jsonl docs/evidence/manual-game/b.jsonl
./target/release/ccc-verify-game --require-agents docs/evidence/agent-draw-incomplete/a.jsonl docs/evidence/agent-draw-incomplete/b.jsonl
```

The second replay checks the agent choices recorded in the events.
It cannot recover the missing model transcripts.
A successful replay does not change the failed harness result.
