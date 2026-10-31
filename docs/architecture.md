# Architecture

Each peer runs the same stack: terminal interface, application state, message protocol, and transport.
The transport carries messages through CPU contention, cache timing, or Unix datagrams for simulation.

```text
Peer A                                  Peer B
Terminal or agent                       Terminal or agent
       |                                       |
Application state                       Application state
       |                                       |
Message protocol                        Message protocol
       |                                       |
Timing transport <--- shared hardware ---> Timing transport
```

## Components

| Component | Responsibility |
| --- | --- |
| `ccc-cli` | Reads terminal input and displays chat or game state. |
| `ccc-app` | Manages sessions, delivery state, and agent processes. |
| `ccc-game` | Checks turns, moves, and game outcomes. |
| `ccc-protocol` | Encodes frames, establishes sessions, and handles acknowledgements and retries. |
| `ccc-channel` | Sends and receives through the selected transport. |
| `ccc-core` | Defines shared types, limits, and timing calculations. |

Each peer has one worker that owns its session and transport.
Agent processes run separately, so model requests do not block protocol polling.

## Transports

- **CPU contention:** the sender alternates between CPU work and idle periods.
- **Cache timing:** the sender creates a memory workload that changes measured timing.
- **Simulation:** peers exchange Unix datagrams to test the software.

The program does not silently change transports after a failure.
Simulation does not test the hardware channel.
Cache-mode timing differences do not isolate cache eviction from scheduling or memory traffic.

## Container boundaries

Manual peers run in separate containers without networking, shared writable mounts, or shared IPC.
The host starts and observes the peers.
It does not relay messages between them.

Hosted agents need external network access for model requests.
Each peer uses a separate bridge, but those bridges allow general egress.
These settings do not prove that all other communication channels are absent.

## Frames and timing

The message protocol uses version 2 frames with a maximum payload of 256 bytes.
Each frame adds 19 bytes of overhead.
The [protocol specification](protocol.md) defines the fields and acceptance rules.

The physical layer adds:

1. A 32-bit `0xAAAAAAAA` preamble.
2. Training signals, carrier periods, and gaps.
3. A length, payload, and CRC.
4. Error correction with 13-bit SECDED codewords and interleaving.

SECDED means single-error correction and double-error detection.
The slot budget is `156 + 13 × n`, where `n` is the number of bytes supplied to the physical layer.
See `crates/ccc-channel/src/phy.rs` for the encoding.

Peers must use the same build.
Older physical formats changed without a version increase.
Current version 2 peers cannot communicate with those version 1 builds.
A wire-format change requires another physical test.

## Message acceptance

The receiver checks and applies a new game message before it sends an acknowledgement.
It acknowledges a duplicate without applying the message again.

The sender changes its board only after the acknowledgement arrives.
If every acknowledgement is lost, the receiver can advance while the sender remains uncertain.
The sender then reports unknown delivery and stops further messages.
It does not assume that both boards agree.

The application checks the role and state before it accepts a game start.
A frozen or aborted game cannot apply later moves.
A restart creates a new session without replaying queued messages.

## Agent processes

Each agent receives a fixed copy of the board, session, and request identity.
The application checks the returned move against the current request and legal turn.

The supervisor limits process lifetime and output.
It terminates the process group after failure, timeout, or cancellation.
A descendant that deliberately escapes that group is outside this guarantee.

## Recorded evidence

The [game verifier](events.md) replays both event files after a run.
It checks legal histories, matching boards, and sender acknowledgements against receiver acceptance.
It can also compare recorded agent choices with accepted moves.

Event replay does not prove physical transport, model authorship, or container isolation.
Those claims require separate evidence from the run.

[Observed results](measurements.md) · [Experiment commands](reproducing.md)
