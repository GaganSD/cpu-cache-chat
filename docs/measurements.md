# Channel measurements

No physical-channel measurement has been recorded yet. Software review found that short receive polls could not complete a physical preamble; hunt state is now preserved across polls. That is not evidence the hardware link works.

Docker/OrbStack was not running when implementation started. Software protocol tests use the labeled `sim` transport only.

| Date | Host | Isolation | Mode | Result |
|---|---|---|---|---|
| — | Apple M2 8 GB / OrbStack | not established | cache | unmeasured |
| — | Apple M2 8 GB / OrbStack | not established | cpu | unmeasured |

When results exist, include: silent-sender control, receiver-blind payloads, bit-error rate, slot length, and whether the claim is cache occupancy vs scheduling correlation.
