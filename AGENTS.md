# Working on cpu-cache-chat

This is a small weekend experiment in hardware-timing communication. Keep changes focused and documentation straightforward; don't turn it into a production-readiness exercise.

- Prefer Rust for the core implementation. Keep tests separate from source.
- Keep Apple-silicon Macs and Linux in mind; avoid platform-specific assumptions.
- Preserve the distinction between cache timing, CPU contention, and simulation. Never silently switch channels.
- The host may launch and observe peers, but must not relay live messages or game state between them.
- Report observed successes and failures. Timing contrast alone isn't proof of a decoded message or cache eviction.
- Keep internal notes, raw measurements, and model transcripts local and ignored. Publish only deliberately curated evidence.
- Use Cargo and npm lockfiles. Run relevant tests before committing.

Useful checks:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
npm ci --prefix agent
npm test --prefix agent
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
```

Physical experiments are manual and timing-sensitive. Run them sequentially; don't start hosted inference or change host settings without authorization.
