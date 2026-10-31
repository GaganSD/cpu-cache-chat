# Findings

Experiments ran on an Apple M2 MacBook with 8 GB RAM, using OrbStack and native ARM64 containers. Manual peers used no networking, private IPC, and no shared writable mounts. Hosted agents used separate egress-enabled bridges.

The useful result: a deliberately modulated timing signal can carry actual messages and game moves between these programs. CPU contention was more practical than the cache workload. Neither experiment isolates a particular cache-eviction mechanism.

## Chat

| Experiment | Observation | Sample |
| --- | --- | --- |
| CPU, 50 ms slots, October 27 | Bidirectional messages delivered and acknowledged; an earlier identical attempt failed to lock. | [Probe failure](evidence/probe-no-lock/receiver.json) |
| Cache, 50 ms slots, October 28 | Silent control and receiver-blind probe passed; both messages delivered. A retransmission was needed. Other cache attempts failed. | [Peer logs and harness](evidence/cache-chat/) |
| CPU, 50 ms slots, October 29 campaign | 7/10 attempts passed, taking 269–959 s for passes. Failures took 71–112 s. | [All ten outcomes](evidence/cpu-chat-50ms/summary.jsonl) |
| CPU, 25 ms slots, later October 29 campaign | 5/5 attempts passed, taking 154–344 s each. All needed retransmissions; controls were shared at campaign level. | [All five outcomes and one peer pair](evidence/cpu-chat-25ms/) |

The campaigns used different builds and control arrangements. Their small counts aren't a controlled speed comparison or an estimate of general reliability. Passing messages doesn't mean every attempt worked.

## Games

- Earlier 50 ms CPU games completed manually and with hosted agents, including wins and a draw.
- A later 25 ms manual-game campaign passed **3/3**, taking **264, 342, and 416 seconds**. The verifier replayed matching five-ply X wins. [Campaign and example](evidence/manual-game/).
- Before that campaign, six retained 25 ms manual-game campaigns recorded **0/3, 0/3, 0/3, 0/3, 0/2, and 0/3**. Startup timing and scheduler noise were recurring problems; the later successes followed harness/timing changes.
- Later hosted-agent campaigns recorded **0/2, 0/3, 0/3, and 0/3** overall passes. The first used a stale image; later builds still had handshake problems. In the final complete campaign, one nine-ply draw passed the event verifier but failed the harness because full transcript capture was missing. [Draw with incomplete evidence](evidence/agent-draw-incomplete/).
- Another agent run was interrupted mid-game and isn't counted as a completed attempt.
- Two older cache-game attempts failed at handshake; no completed cache-mode game was established.

An initial five-attempt CPU chat campaign also recorded **0/5** with harness problems; it isn't silently folded into the later 7/10. [Campaign summaries](evidence/campaigns/) retain these unsuccessful outcomes as well as the later ones.

## What the failures taught us

- **Contrast isn't decoding.** Early runs showed large active/idle timing differences but couldn't acquire a frame.
- **Scheduling is part of the channel.** A pause can erase samples or lose a transmit slot. Training, interleaving, error correction, bounded retries, and settling between container runs helped, but didn't eliminate noise.
- **Receiving isn't the same as confirmed delivery.** Some messages reached the receiver while every ACK was lost. The sender reported `delivery unknown`; a game stopped rather than pretending both boards agreed.
- **Evidence collection can fail independently.** A completed game, captured model transcript, and successful harness exit are different observations.
- **CPU contention isn't proof of cache eviction.** The cache workload also creates memory traffic and scheduling effects.

## What these results don't establish

The public samples are historical runs from several builds, not fresh tests of every subsequent change. Peers need matching wire formats. Results on other Macs or Linux hosts may differ.

The game verifier checks event consistency. Its `physical_channel_verified: false` and `isolation_verified: false` fields are intentional: event replay alone can't establish either. Hosted providers saw both boards, bridge egress wasn't destination-restricted, and refused peer connections weren't proof of network isolation. Offline agent play wasn't measured.

Full raw campaigns, Docker dumps, internal model-review reports, and model sessions are kept locally rather than published by default. The [curated evidence index](evidence/README.md) documents source paths, checksums, and any sanitization. Model-review reports were automated source/log reviews, not independent human certification; their useful observations are reflected here without treating them as experimental evidence.

See [reproduction commands](reproducing.md) to run your own tests.
