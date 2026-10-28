# Protocol status

The protocol is experimental and is not authenticated or encrypted. Wire version
2 is incompatible with version 1. The handshake checks the configured
application kind (chat or game); incompatible peers cannot establish a link.
Sync and SyncAck use `seq` 1 for chat and 2 for game. Other controls have no
payload; reserved flags must be zero and each frame type's sequence and ACK
fields are validated. Session IDs are drawn from OS entropy (`/dev/urandom` on
Mac and Linux); failure to obtain entropy terminates rather than using a weak
fallback. Session IDs are nonzero, are not authentication, and can collide.

Token handoffs use a session-wide monotonic 16-bit identity in Heartbeat's
`seq`. The sender relinquishes before transmission, and retries only that exact
offer until an Ack with matching `seq` and zero `ack_seq` arrives. A receiver
accepts only the next identity, confirms duplicate current offers without
regranting, and ignores stale or unexpected offers. A returned token advances
the identity, so a delayed previous offer cannot regrant it. Identity exhaustion
fails the session. No side speculatively reclaims a token. Tokenless inactivity
has an overall deadline based on the configured response/recovery budgets and
retry count (at least 500 ms for simulation, 30 seconds for physical); an
unresponsive peer eventually causes `Failed`. This deadline assumes timely
calls to `drive` and bounded wire operations, and may require tuning for slow
peers. Default simulation uses 250 ms response waits and a 5 s connection budget, so tokenless absence expires in about 2 seconds with five retries. Physical defaults deliberately remain much longer: tokenless recovery can take several minutes at 50 ms slots. Those are bounds, not promises of delivery.

A Data frame is retried with the same sequence and payload. If its ACK remains
missing after the retry budget, the sender reports `Delivery::Unknown`, retains
the unresolved frame and enters `Failed`. It must be restarted with a fresh
session; queued messages are not automatically replayed. The peer may already
have applied the message. Each direction may enqueue at most 65,535 messages
per session; sequence numbers never wrap. The outbound queue, including any
inflight message, is limited to 256; enqueue returns an error on exhaustion or
capacity rather than silently discarding data.

`Link::drive_with_accept` invokes a synchronous callback on a new, decoded,
in-sequence message before its ACK. The callback must validate and apply the
message without blocking on model calls or I/O. Returning an error sends a NACK
instead. A duplicate of an accepted sequence is ACKed without another callback.
The legacy `drive` wrapper accepts every syntactically valid message; existing
applications using it do **not** obtain application-level acceptance. A NACK
is currently retried and eventually reported as unknown, not a distinct
application rejection. The application must separately enforce its lifecycle.
