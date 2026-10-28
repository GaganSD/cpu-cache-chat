# Contributing

Use Rust 1.96.1, Node 22.19 or newer, and the lockfiles. Keep tests separate from source. Before proposing changes run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, `cargo test --workspace --locked`, and `cd agent && npm ci && npm test`. Run shell fixtures and simulation as described in CI. Physical tests are manual and do not run on untrusted CI; report failures and environment as well as successes. Never include credentials or private logs in reports. See [release preparation](docs/release.md).
