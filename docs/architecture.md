# Architecture (current implementation overview)

The CLI joins application sessions, protocol framing and a selected transport. `ccc-game` checks moves; `ccc-app` manages chat/game state; `ccc-protocol` handles frames and link state; `ccc-channel` implements timing transports and Unix-datagram simulation. Simulation is not isolation or a hardware result.

## Boundary

Manual physical tests use separate Docker containers, normally network-none, private IPC and no shared writable mounts. The host launches and observes them, not a payload relay. Hosted agent containers instead require external network access; separate bridges do not restrict egress to the inference provider. Neither Docker configuration proves absence of other covert channels. The cache workload's timing contrast is not proof of eviction.

## Wire and reliability

Protocol frames in `crates/ccc-protocol/src/codec.rs` are `C3 C4 | version=1 | type | flags | session:u32le | seq:u16le | ack:u16le | length:u16le | payload | crc32:u32le`; the CRC covers the preceding bytes and payload is limited to 256 bytes. Consult the source and tests for current state transitions and semantic constraints; these are still being hardened and are not a security guarantee.

The physical layer uses a 32-bit `0xAAAAAAAA` preamble, training/carrier and gaps, then a length, payload and CRC encoded with 13-bit SECDED codewords and interleaving. The slot budget is 156 + 13 per payload byte; see `ccc-core` and `ccc-channel/src/phy.rs` for the exact implementation. This differs from earlier physical formats even when protocol version numbers match. Peers must use the same build. New physical results are required after wire changes.

The selected cache or CPU mode is explicit; a failed physical link must not silently become simulation. Missing acknowledgements can make delivery unknown. Consult [measurements](measurements.md) for the distinction between historical evidence and current release gates.
