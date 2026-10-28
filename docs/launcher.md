# Demo launcher

Build the appropriate local image first (`docker/build.sh`; use the agent image build for hosted play). Docker Engine on Linux or Docker Desktop/OrbStack on Apple Silicon, Python 3, and a Docker guest CPU shared by the containers are required.

```sh
scripts/demo.sh doctor
scripts/demo.sh chat --channel cpu
scripts/demo.sh game --channel cache --cpu 0
CCC_HOSTED_OPT_IN=1 scripts/demo.sh agents --channel cpu --provider opencode-go
CCC_HOSTED_OPT_IN=1 scripts/demo.sh agents --channel cpu --provider vercel-ai-gateway
scripts/demo.sh stop --run-id RUN_ID
```

Attach to each printed container in a separate terminal; Ctrl-p Ctrl-q detaches. The run persists until stopped. `--image` selects a locally available image; the launcher resolves its immutable ID before creation. The named `reference-50ms` preset is shared by every mode. A nominal 19-byte control frame takes about 20 seconds; retries can take much longer. No silent switch between cache and CPU occurs.

By default a bounded 300-second, receiver-blind, 32-bit physical probe precedes launch. Its matching bytes are not a reliability guarantee. `--skip-preflight` explicitly skips it, for tests or informed troubleshooting; this does not establish a working channel. A failed probe stops the launch. `doctor` checks Docker reachability and reports guest CPU count, not image, credential, or hardware-channel readiness. CPU indices are checked against the Docker daemon guest's CPU count, not the Mac host's count, and a disposable bounded container verifies that Docker accepts the chosen cpuset.

Manual peers have no network. Hosted peers get separate egress-enabled bridges, **not destination-restricted firewalls**: peer network isolation is unverified. Board prompts leave the machine and provider charges may apply. Hosted operation requires explicit `CCC_HOSTED_OPT_IN=1` and `OPENCODE_GO_API_KEY` or `AI_GATEWAY_API_KEY` (also accepts `VERCEL_AI_GATEWAY_API_KEY`). The fixed models are `deepseek-v4.1-flash` with low reasoning and `reflection/beam-free`, respectively. Offline agents are not provisioned and fail before Docker resource creation. Credentials are passed by environment variable name, never placed in manifests or printed by the launcher; Docker stores container environment and privileged Docker users can inspect it. Protect Docker access and provider keys.

Each run has a random ownership label and a private manifest in `~/.cpu-cache-chat/runs` (override with `CCC_RUN_DIR`). Stop and failure cleanup select only that run label; retain the manifest if cleanup fails and retry stop. Deleting a manifest prevents the launcher from stopping that run. Containers use a non-root user, read-only root, private IPC, no extra capabilities, no-new-privileges, and memory, PID and CPU bounds. These controls and inspection do not establish hardware-channel success or live network isolation.

## Run retention and evidence

Manifests are **retained** after stop and failure, recording `status` (`running`, `stopped`, `failed`), `start`/`end`, probe result (`passed`/`skipped`/`pending`), hosted peer-path policy, the configuration, and the resolved image ID plus best-effort provenance (source/revision labels and bounded `--version` output; the launcher never dumps container `Env`). `stop` is idempotent: rerunning it on a cleaned-up run reports the retained state without touching anything, and manifests with a mismatched run ID or ownership label are rejected.

Before any owned container is removed (on stop or launch failure), the launcher captures split Docker stdout/stderr logs (TTY containers merge both into stdout), `docker cp` of the CLI evidence file `--events-path /tmp/game.events.jsonl`, and, for hosted runs, the agent raw transcript `--env CCC_AGENT_TRANSCRIPT_PATH=/tmp/agent-transcript.log` (with `CCC_AGENT_DEBUG=1`). A transcript that reaches the adapter's 16 MiB bound is marked `truncated`. Missing events or missing/truncated transcripts mark the run `incomplete` with explicit reasons in the manifest; no record is invented for absent files.

Artifacts live under `<run-id>/` next to the manifest and each file is labeled **REVIEW BEFORE SHARING**: redaction (live credential environment values plus bearer/key patterns) is best-effort and **not guaranteed secret-free**. Retention is bounded: a 64 MiB per-run quota (overflow is truncated and recorded as incomplete) and a total budget of 8 runs / 512 MiB; a launch that would exceed the budget is refused and the launcher never deletes unknown files, so prune retained runs manually.
