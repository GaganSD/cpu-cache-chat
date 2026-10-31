# Contributing

This is a weekend experiment; small fixes, clear explanations, and results from other machines are welcome.

Use Rust 1.96.1, Node 22.19+ for agent tests, and the lockfiles. Keep tests separate from source and preserve compatibility with Apple-silicon Macs and Linux.

Run the relevant [software checks](docs/reproducing.md#software-only-checks) before proposing a change. Physical tests are manual: report the host, configuration, failures, and successes. Keep raw logs local and publish only reviewed samples.

See [architecture](docs/architecture.md), [protocol](docs/protocol.md), and [experiment commands](docs/reproducing.md).
