# Agent player

The player reads one JSON object from stdin (close stdin after writing):

```json
{"board":"123456789","turn":"X","legal":[1,2,3,4,5,6,7,8,9]}
```

On success it writes a single JSON object such as `{"square":5,"source":"pi"}` to stdout. Errors go to stderr and result in a nonzero exit. Rust must independently validate the returned move. The process runs one prompt, allows at most three illegal submissions, and has a 90-second hard deadline. It has no default tools or discovered extensions; the only active tools are `get_board` and `submit_move`.

Provision Node.js 22.19 or later and install the dependencies with `npm install` in `agent/` before the offline run. Two providers are supported:

- **llama (default, offline):** start a local llama.cpp server separately, with a tool-capable model and `--jinja`, bound to `127.0.0.1`. Set `LLAMA_BASE_URL` to its HTTP root (default `http://127.0.0.1:8080`, optionally ending in `/v1`) and `LLAMA_MODEL` to the server's model ID (default `local-model`). Only literal loopback HTTP URLs are accepted. No provider key is needed. Model weights and dependencies must already be present; this player does not download them. Use a server configuration without remote model autoload or downloads.
- **opencode-go (hosted):** set `CCC_AGENT_PROVIDER=opencode-go`, pass the key via `OPENCODE_GO_API_KEY`, and optionally choose `CCC_AGENT_MODEL` from the allowlist in `src/game.mjs` (default `glm-5.3-flash`). The player fetches only the single provider origin and the container needs network egress to it; pair this with a per-peer network that has no route to the other player (see `scripts/e2e-docker-agent.sh`). The key must come from the environment, never the image.

In both cases the Pi SDK credential store needs a writable `HOME` (the agent image sets `HOME=/tmp`).

`npm test --prefix agent` covers parsing, illegal moves, and a separate test-only fixture. `play.mjs` has no scripted fallback. With dependencies installed the suite also exercises the Pi SDK against a mock loopback server. No real model is needed.
