# Protocol status

The protocol is experimental and is not authenticated or encrypted. Both peers
must use compatible protocol and physical-layer builds; a failed physical lock
cannot reliably distinguish an incompatible build from noise.

A Data frame is retried with the same sequence and payload. If its ACK remains
missing after the retry budget, the sender reports `Delivery::Unknown`, retains
the unresolved frame and enters `Failed`. It must be restarted with a fresh
session; queued messages are not automatically replayed. The peer may already
have applied the message.

`Link::drive_with_accept` invokes a synchronous callback on a new, decoded,
in-sequence message before its ACK. The callback must validate and apply the
message without blocking on model calls or I/O. Returning an error sends a NACK
instead. A duplicate of an accepted sequence is ACKed without another callback.
The legacy `drive` wrapper accepts every syntactically valid message; existing
applications using it do **not** obtain application-level acceptance. A NACK
is currently retried and eventually reported as unknown, not a distinct
application rejection. The application must separately enforce its lifecycle.

Known limitations: token handoffs have no explicit identity or total recovery
bound; delayed or duplicate control frames are not safely distinguished. The
wire codec does not yet enforce all control-field semantics, sequence numbers
wrap, session identifiers are not generated using a secure random source, and
the handshake does not negotiate application kind. These gaps preclude a
claim of complete protocol correctness.
