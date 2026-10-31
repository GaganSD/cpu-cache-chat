# Sample evidence

A small selection of historical output from the M2/OrbStack experiments. These are captured results, not illustrative output or fresh runs of the current checkout.

| Directory | What it preserves |
| --- | --- |
| [cache-chat](cache-chat/) | A 50 ms cache-mode chat: silent control, decoded probe, both peer logs, configuration, and passing harness. Includes a retransmission. |
| [probe-no-lock](probe-no-lock/) | An earlier CPU receiver timeout. Its legacy zero error field isn't a measured BER. |
| [cpu-chat-50ms](cpu-chat-50ms/) | All ten campaign outcomes (7 passes) and one failed receiver probe. |
| [cpu-chat-25ms](cpu-chat-25ms/) | All five campaign outcomes (5 passes with retries), plus one peer pair and its control-bundle reference. The full control bundle isn't included. |
| [manual-game](manual-game/) | All three campaign outcomes, plus one five-ply game's event histories, configuration, harness, and verifier verdict. |
| [agent-draw-incomplete](agent-draw-incomplete/) | All three campaign outcomes (no overall passes), plus the completed nine-ply draw's events and verdict. Transcript capture failed, so the harness still failed. |
| [campaigns](campaigns/) | Every retained campaign summary, including failed, harness-bug, stale-image, and partial campaigns. Only completed attempts with summary rows are counted. |

## Provenance and edits

[manifest.json](manifest.json) records the original relative source path, original SHA-256, published SHA-256, and transformations for each file. The source revision identifies the original snapshot before the repository's history cleanup; it isn't a link to an accessible historical commit.

- Absolute checkout paths are replaced with `<checkout>`.
- Output files are sometimes renamed to `.txt` or `.json` for readability.
- The agent harness excerpt ends before its diagnostic peer-log tails; its success, warnings, verifier result, and overall failure are preserved.
- Other sampled file contents are unchanged. Session IDs, image IDs, recorded revisions, error fields, and failures are retained.
- Full raw campaigns, Docker inspection dumps, model transcripts, and automated review reports are local-only. These samples aren't a complete archive or an independent audit.

Historical harness messages about isolation or physical operation reflect what those harnesses printed, not additional proof. In particular, a verifier's `ok: true` means the event histories agree; it doesn't verify transport or isolation. Read the [findings](../measurements.md) alongside the output.

## Replay the sampled games

From the repository root:

```sh
cargo build --locked --release -p ccc-game
./target/release/ccc-verify-game docs/evidence/manual-game/a.jsonl docs/evidence/manual-game/b.jsonl
./target/release/ccc-verify-game --require-agents docs/evidence/agent-draw-incomplete/a.jsonl docs/evidence/agent-draw-incomplete/b.jsonl
```

The second replay checks agent choices recorded in the events. It cannot recover the missing inference transcripts or turn the failed harness run into a pass.
