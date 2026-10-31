# Agent player

The player asks a model to select one tic-tac-toe move.
Rust checks the returned move before the application can use it.
The player has no scripted fallback.

## Input and output

Write one JSON object to stdin.
Then close stdin.

```json
{"board":"123456789","turn":"X","legal":[1,2,3,4,5,6,7,8,9]}
```

On success, the player writes one JSON object to stdout:

```json
{"square":5,"source":"pi"}
```

Errors go to stderr.
An error returns a nonzero exit status.

Each process handles one board request with a 90-second deadline.
It permits at most three illegal submissions.
Its only active tools are `get_board` and `submit_move`.
It does not load default tools or discovered extensions.

## Install dependencies

Use Node.js 22.19 or later.

```sh
npm ci --prefix agent
```

The Pi SDK needs a writable `HOME` for its credential store.
The agent image uses `HOME=/tmp`.

## Providers

Set `CCC_AGENT_PROVIDER` to select a provider.
The default is `llama`.

### Local llama.cpp

1. Start a separate llama.cpp server with a tool-capable model.
2. Enable `--jinja`.
3. Bind the server to `127.0.0.1`.
4. Set the endpoint and model variables if their defaults do not apply.

| Variable | Default |
| --- | --- |
| `LLAMA_BASE_URL` | `http://127.0.0.1:8080` |
| `LLAMA_MODEL` | `local-model` |

The base URL can end in `/v1`.
The player accepts only HTTP URLs with literal loopback addresses.
It needs no provider key.

Install model weights and dependencies before the run.
The player does not download them.
Disable remote model loading and downloads in the server configuration.

The experiments did not test offline setup or resource use with two local models.

### OpenCode Go

Set these environment variables:

| Variable | Value |
| --- | --- |
| `CCC_AGENT_PROVIDER` | `opencode-go` |
| `OPENCODE_GO_API_KEY` | Your provider key |
| `CCC_AGENT_MODEL` | `deepseek-v4.1-flash`, if set |

Only `deepseek-v4.1-flash` is allowed.
It uses low reasoning.
The model list is in `src/game.mjs`.

### Vercel AI Gateway

Set these environment variables:

| Variable | Value |
| --- | --- |
| `CCC_AGENT_PROVIDER` | `vercel-ai-gateway` |
| `AI_GATEWAY_API_KEY` | Your provider key |
| `CCC_AGENT_MODEL` | `reflection/beam-free`, if set |

Only `reflection/beam-free` is allowed.
Its output limit is 8,192 tokens.
The same 90-second deadline applies.
The provider can fail or change model availability.

### Hosted access

Both hosted modes send board prompts to an external provider.
Host launches require explicit permission through `CCC_HOSTED_OPT_IN=1`.

Supply keys through the environment.
Do not add them to images.

The fetch wrapper permits requests only to the configured provider origin.
This application check is not a network firewall.
The container bridges still permit general egress.

## Transcripts

Provider response data has a 1 MiB limit.
Each request's debug transcript also has a 1 MiB limit.
Exceeding either limit fails the request and marks its evidence incomplete.

Set `CCC_AGENT_TRANSCRIPT_PATH` to retain full redacted transcripts.
The target must be a private regular file owned by the current user.
The total file limit is 16 MiB.

The supervised agent process writes the transcript.
The transport worker does not write it.
Collect the file before removing the container.

Without a transcript path, the player writes debug output to stderr.
The interface can truncate that output.
Do not treat truncated output as a complete model transcript.

## Tests

```sh
npm ci --prefix agent
npm test --prefix agent
```

The Pi tests use a mock server and explicitly select the local provider.
They fail if dependencies are missing.
They do not need a real model.
