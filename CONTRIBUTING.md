# Contributing

Small fixes, clear explanations, and results from other machines are welcome.

## Code changes

- Use Rust 1.96.1.
- Use Node 22.19 or later for agent tests.
- Use the dependency lockfiles.
- Keep tests separate from source.
- Preserve compatibility with Apple-silicon Macs and Linux.

Run the relevant [software checks](docs/reproducing.md#software-only-checks) before proposing a change.

## Experiment results

Physical tests are manual.
Report the host, configuration, failures, and successes.
Keep raw logs local.
Publish only reviewed samples.

[Architecture](docs/architecture.md) · [Protocol](docs/protocol.md) · [Experiment commands](docs/reproducing.md)
