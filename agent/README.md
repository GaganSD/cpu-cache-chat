# Agent player

The player reads one JSON object from stdin (close stdin after writing):

```json
{"board":"123456789","turn":"X","legal":[1,2,3,4,5,6,7,8,9]}
```

On success it writes a single JSON object such as `{"square":5,"source":"pi"}` to stdout. Errors go to stderr and result in a nonzero exit. Rust must independently validate the returned move. The process runs one prompt, allows at most three illegal submissions, and has a 90-second hard deadline. It has no default tools or discovered extensions; the only active tools are `get_board` and `submit_move`.

Provision Node.js 22.19 or later and install the dependencies with `npm install` in `agent/` before the offline run. Provision and start a local llama.cpp server separately, with a tool-capable model and `--jinja`, bound to `127.0.0.1`. Set `LLAMA_BASE_URL` to its HTTP root (default `http://127.0.0.1:8080`, optionally ending in `/v1`) and `LLAMA_MODEL` to the server's model ID (default `local-model`). Only literal loopback HTTP URLs are accepted. No provider key is needed. Model weights and dependencies must already be present; this player does not download them. Use a server configuration without remote model autoload or downloads.

For tests only, `CCC_ALLOW_FIRST_LEGAL=1 node agent/src/play.mjs` chooses the first listed legal square without importing Pi or accessing a model. It is not a demo player. `npm test --prefix agent` tests parsing, illegal moves, and the hook; with dependencies installed it also exercises the real Pi SDK against a mock loopback server. No real model is needed.
