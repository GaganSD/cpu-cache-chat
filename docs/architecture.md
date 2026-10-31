# Architecture (current implementation overview)

The CLI joins application sessions, protocol framing and a selected transport. `ccc-game` checks moves; `ccc-app` manages chat/game state; `ccc-protocol` handles frames and link state; `ccc-channel` implements timing transports and Unix-datagram simulation. Simulation is not isolation or a hardware result.

## Boundary

Manual physical tests use separate Docker containers, normally network-none, private IPC and no shared writable mounts. The host launches and observes them, not a payload relay. Hosted agent containers instead require external network access; separate bridges do not restrict egress to the inference provider. Neither Docker configuration proves absence of other covert channels. The cache workload's timing contrast is not proof of eviction.

## Wire and reliability

Protocol frames in `crates/ccc-protocol/src/codec.rs` are `C3 C4 | version=2 | type | flags | session:u32le | seq:u16le | ack:u16le | length:u16le | payload | crc32:u32le`; the CRC covers the preceding bytes and payload is limited to 256 bytes. The encoded overhead is 19 bytes. Version 2 rejects reserved flags and invalid control fields, negotiates chat versus game, and identifies token offers monotonically. See [the protocol contract](protocol.md) for exact transitions, limits, and compatibility.

Each peer has one wire/session owner. Agent subprocess jobs receive immutable board/session/request snapshots and execute independently, so inference does not block protocol polling. A returned move must still match the live generation and legal turn. Subprocess output and lifetime have quotas; the supervisor terminates its owned process group on failure, timeout, or cancellation. A descendant deliberately escaping that group is not covered by a sandbox guarantee.

The receiver validates and applies a new game message synchronously before sending its acceptance ACK; duplicates are acknowledged without applying twice. The sender commits its tentative board only on that ACK. Lost ACKs can therefore leave the receiver ahead: the sender retains an uncertain move and fails closed rather than sending a later move or fabricating board agreement. Restart means a new session, not automatic replay. Start is role/state checked, and freeze/abort prevents later move mutation.

Post-run [JSONL evidence verification](events.md) replays both complete legal histories, checks sender ACK against receiver acceptance, and optionally matches local agent choices. It does not prove physical transport, inference authorship, or isolation; those require independent runtime evidence.

The physical layer uses a 32-bit `0xAAAAAAAA` preamble, training/carrier and gaps, then a length, payload and CRC encoded with 13-bit SECDED codewords and interleaving. The slot budget is 156 + 13 per payload byte; see `ccc-core` and `ccc-channel/src/phy.rs` for the exact implementation. Historical physical formats changed while still using version 1; the current version-2 peers cannot interoperate with those binaries. Peers must use the same build. New physical results are required after wire changes.

The selected cache or CPU mode is explicit; a failed physical link must not silently become simulation. Missing acknowledgements can make delivery unknown. See [findings](measurements.md) for observed successes, failures, and the limits of those experiments.
