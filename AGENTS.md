# Working on cpu-cache-chat

This is a weekend experiment in communication through hardware timing.
Keep changes focused.
Use direct technical explanations, not production-readiness checklists.

## Code

- Prefer Rust for the core implementation.
- Keep tests separate from source.
- Preserve compatibility with Apple-silicon Macs and Linux.
- Avoid platform-specific assumptions.
- Use Cargo and npm lockfiles.
- Run relevant tests before committing.

## Experiments

- Keep cache timing, CPU contention, and simulation distinct.
- Never silently change channels.
- Allow the host to start and observe peers.
- Do not let the host relay live messages or game state between peers.
- Report observed successes and failures.
- Do not treat timing contrast as proof of a decoded message or cache eviction.
- Keep internal notes, raw measurements, and model transcripts local and ignored.
- Publish only reviewed evidence.

Physical tests are manual and timing-sensitive.
Run them sequentially.
Do not start hosted inference or change host settings without authorization.

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
npm ci --prefix agent
npm test --prefix agent
python3 -m unittest discover -s scripts/tests -p 'test_*.py'
```
