# Game event format

Each peer records one game as JSON Lines, or JSONL.
Each line contains one event.
The verifier replays both files and compares the results.

## Run the verifier

```sh
ccc-verify-game A.jsonl B.jsonl
ccc-verify-game --require-agents A.jsonl B.jsonl
```

The verifier prints one JSON object to stdout.
A failure returns a nonzero exit status and this structure:

```json
{"ok":false,"error":"reason"}
```

A successful result includes these fields:

| Field | Meaning |
| --- | --- |
| `ok` | Both event histories passed the checks. |
| `session` | Shared session ID |
| `board` | Final board |
| `ply` | Number of accepted moves |
| `outcome` | `x_win`, `o_win`, or `draw` |
| `agents_verified` | The verifier checked recorded agent choices. |
| `physical_channel_verified` | Always `false`. Event replay cannot check the transport. |
| `isolation_verified` | Always `false`. Event replay cannot check isolation. |

Keep physical-channel and isolation evidence separately.
The verifier relies on truthful events.
It does not authenticate their source.

## File limits

| Requirement | Limit |
| --- | --- |
| Encoding | UTF-8 |
| File size | 16,384 bytes |
| Line size | 1,024 bytes, excluding the newline |
| Event count | 21 |
| Sessions per file | 1 |

Every object needs a terminating newline, including the last object.
The parser rejects blank lines and unknown, missing, or duplicate fields.

## Event order

A file contains one session event, move events, one final-board event, and one stopped event.
Agent-choice events can appear between moves.

### 1. Session

The first event identifies the session and peer role.

```json
{"type":"session","version":1,"session":42,"role":"A"}
```

The session is a nonzero unsigned 32-bit integer.
The role is `A` or `B`.

### 2. Moves and agent choices

A move event records an accepted move.

```json
{"type":"move","ply":0,"square":1,"mark":"X","sequence":2,"acceptance":"local_ack"}
```

| Field | Rule |
| --- | --- |
| `ply` | Zero-based move index |
| `square` | Integer from 1 through 9 |
| `mark` | `X` or `O` |
| `sequence` | Nonzero unsigned 16-bit integer |
| `acceptance` | `local_ack` for the sender, or `peer_accepted` for the receiver |

The sender records `local_ack` only after it receives the application's acceptance ACK.
The receiver records `peer_accepted` when it applies the move.

Both files must record matching moves with opposite acceptance values.
Each sender's sequence numbers must increase.

An agent-choice event records the local agent's selected square.

```json
{"type":"agent_choice","ply":0,"square":1}
```

Each choice must describe the current local turn.
It must precede its matching move.
Choices cannot repeat a ply.

With `--require-agents`, every accepted local move must match a recorded choice.
The order must also match.

### 3. Final board

The final-board event records the completed game.

```json
{"type":"final","board":"XXXOO6789","ply":5,"outcome":"x_win"}
```

The board string contains nine cells.
An empty cell retains its square number.
The outcome is `x_win`, `o_win`, or `draw`.

### 4. Stopped

The last event records successful completion.

```json
{"type":"stopped","successful":true,"evidence_complete":true}
```

This event states that the process and worker completed successfully with complete records.
It does not replace the other required events or legal replay.

## Checks across peers

The two files must contain:

1. The same nonzero session ID.
2. Opposite peer roles.
3. Matching legal move histories.
4. Matching completed boards and outcomes.

## Stdout copy

`game --plain --events-stdout` also prints each event to stdout.
Each event starts on a separate line with the exact prefix `CCC_EVENT `.

To extract one peer's JSONL file:

1. Select lines that start with `CCC_EVENT `.
2. Remove that exact prefix.
3. Remove one trailing carriage return when a TTY added CRLF line endings.
4. Keep the terminating newline.

The extracted bytes match the `--events-path` file when both outputs contain the complete stream.

The plain frontend prints these events.
The worker sends them through a bounded update queue.
Queue overflow causes a fatal error and marks the run unsuccessful.

`--events-stdout` requires game mode and `--plain`.
The stdout copy remains available through collected container logs after exit.
It provides a fallback when the event file uses temporary memory storage, or tmpfs, that disappears after exit.
