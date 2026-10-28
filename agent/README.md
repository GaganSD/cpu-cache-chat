# Agent player

The player reads one JSON object from stdin (close stdin after writing):

```json
{"board":"123456789","turn":"X","legal":[1,2,3,4,5,6,7,8,9]}
```

On success it writes a single JSON object such as `{"square":5,"source":"pi"}` to stdout. Errors go to stderr and result in a nonzero exit. Rust must independently validate the returned move. The process runs one prompt, allows at most three illegal submissions, and has a 90-second hard deadline. It has no default tools or discovered extensions; the only active tools are `get_board` and `submit_move`.

Provision Node.js 22.19 or later and install the locked dependencies with `npm ci --prefix agent`. The adapter supports three provider configurations; offline model provisioning and two-instance resource viability remain unvalidated:

- **llama (default, offline):** start a local llama.cpp server separately, with a tool-capable model and `--jinja`, bound to `127.0.0.1`. Set `LLAMA_BASE_URL` to its HTTP root (default `http://127.0.0.1:8080`, optionally ending in `/v1`) and `LLAMA_MODEL` to the server's model ID (default `local-model`). Only literal loopback HTTP URLs are accepted. No provider key is needed. Model weights and dependencies must already be present; this player does not download them. Use a server configuration without remote model autoload or downloads.
- **opencode-go (hosted):** set `CCC_AGENT_PROVIDER=opencode-go`, pass the key via `OPENCODE_GO_API_KEY`, and optionally choose `CCC_AGENT_MODEL` from the allowlist in `src/game.mjs` (only `deepseek-v4.1-flash`, low reasoning). The fetch wrapper restricts requests to the configured origin, but the container bridges allow general egress. The key must come from the environment, never the image.
- **vercel-ai-gateway (hosted):** set `CCC_AGENT_PROVIDER=vercel-ai-gateway` and `AI_GATEWAY_API_KEY`. Only `reflection/beam-free` is allowed. Its output cap is 8,192 tokens under the same 90-second deadline; the provider can still fail or change availability. Both hosted modes disclose board prompts and require explicit host-launch opt-in.

In all cases the Pi SDK credential store needs a writable `HOME` (the agent image sets `HOME=/tmp`).

Provider response data and per-request debug transcripts each have a 1 MiB quota; exceeding either fails the request and marks evidence incomplete. Set `CCC_AGENT_TRANSCRIPT_PATH` to retain redacted full transcripts in a private owned regular file (16 MiB total cap). This write happens in the supervised agent subprocess, not the physical wire owner. The launcher/harness should collect that file before removing the container. Without a transcript path, debug stderr can be visibly truncated by the UI and is not complete model evidence. The fetch restriction is an application guard, not a network firewall. The hosted provider receives board prompts.

Run `npm ci --prefix agent` and `npm test --prefix agent`; the Pi mock-server tests fail if dependencies are missing and explicitly select the local provider. `play.mjs` has no scripted fallback. No real model is needed.
