# Message protocol

The protocol carries chat messages and game moves.
An acknowledgement, or ACK, confirms that a peer accepted a message.
The protocol does not authenticate peers or encrypt messages.

## Frame format

All multibyte integers use little-endian encoding.

| Field | Size | Meaning |
| --- | --- | --- |
| Magic | 2 bytes | `C3 C4` |
| Version | 1 byte | `2` |
| Type | 1 byte | Frame type |
| Flags | 1 byte | Reserved bits must be zero. |
| Session | 4 bytes | Session identifier |
| `seq` | 2 bytes | Message sequence or control identity |
| `ack_seq` | 2 bytes | Acknowledged message sequence |
| Length | 2 bytes | Payload length |
| Payload | 0–256 bytes | Application data |
| CRC-32 | 4 bytes | Checksum of all preceding frame bytes |

A frame adds 19 bytes of overhead.
The decoder checks sequence and acknowledgement fields against the frame type.
See `crates/ccc-protocol/src/codec.rs` for the field checks.

## Session startup

Version 2 cannot communicate with version 1.
The handshake also checks the application type.
A chat peer cannot establish a session with a game peer.

`Sync` and `SyncAck` use `seq=1` for chat and `seq=2` for games.
Other control frames carry no payload.

The program reads session IDs from `/dev/urandom` on macOS and Linux.
It stops if that read fails.
IDs are nonzero, but they can collide.
A session ID does not authenticate a peer.

## Token handoff

The token determines which peer can send data.
Each handoff uses an increasing 16-bit identity for the session.
The `Heartbeat.seq` field carries that identity.

1. The sender releases the token before it transmits the offer.
2. The receiver accepts only the next identity.
3. The receiver confirms the offer with matching `Ack.seq` and `ack_seq=0`.
4. The sender retries only that offer until it receives the confirmation.

The receiver confirms a duplicate of the current offer without granting the token again.
It ignores stale and unexpected offers.
Returning the token advances the identity.
A delayed offer from an earlier handoff cannot grant the token again.

The session fails if the next identity exceeds the 16-bit limit.
Neither peer assumes token ownership after an uncertain handoff.

## Timeouts

A peer without the token has an overall inactivity deadline.
The deadline uses the larger response or recovery budget, multiplied by `max_retries + 3`.
Its minimum is 500 ms for simulation and 30 seconds for physical channels.

| Configuration | Default timing |
| --- | --- |
| Simulation response wait | 250 ms |
| Simulation connection budget | 5 seconds |
| Simulation token inactivity | About 2 seconds with five retries |
| Physical token recovery | Can take several minutes at 50 ms slots |

These limits assume timely calls to `drive` and bounded transport operations.
Slow peers may need different settings.
A deadline limits waiting.
It does not promise delivery.
An unresponsive peer eventually causes the `Failed` state.

## Data delivery

A retry uses the same sequence and payload as the original `Data` frame.

After the acknowledgement budget expires, the sender:

1. Reports `Delivery::Unknown`.
2. Retains the unresolved frame.
3. Enters the `Failed` state.

The receiver may already have applied that message.
Restart with a new session before sending more data.
The program does not replay queued messages automatically.

Each direction accepts at most 65,535 messages per session.
Sequence numbers do not wrap.
The outbound queue holds at most 256 messages, including the unresolved message.
Enqueue returns an error when either limit is reached.
It does not discard messages silently.

## Application acceptance

`Link::drive_with_accept` calls the application before acknowledging a new decoded message with the expected sequence.

The callback must:

1. Check the message.
2. Apply the message.
3. Return without waiting for model calls or I/O.

An error causes a negative acknowledgement, or NACK.
The protocol acknowledges a duplicate accepted sequence without calling the application again.

The older `drive` wrapper accepts every syntactically valid message.
It does not check application rules.
The application must enforce game state and lifecycle rules separately.

The sender currently retries after a NACK.
If retries fail, it reports unknown delivery rather than a separate application-rejection result.
