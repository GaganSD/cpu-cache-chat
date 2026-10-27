import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

let hasPi = true;
try { import.meta.resolve("@earendil-works/pi-coding-agent"); import.meta.resolve("typebox"); } catch { hasPi = false; }

test("Pi sends only game tools and handles illegal then legal tool calls", { skip: !hasPi, timeout: 20000 }, async () => {
  const requests = [];
  const server = createServer(async (req, res) => {
    let body = "";
    for await (const chunk of req) body += chunk;
    const parsed = JSON.parse(body);
    requests.push({ path: req.url, body: parsed });
    const square = requests.length === 1 ? 1 : 3;
    res.writeHead(200, { "content-type": "text/event-stream" });
    const chunk = { id: `mock-${requests.length}`, object: "chat.completion.chunk", created: 1,
      model: "local-model", choices: [{ index: 0, delta: { role: "assistant", tool_calls: [{ index: 0,
        id: `call-${requests.length}`, type: "function", function: { name: "submit_move", arguments: JSON.stringify({ square }) } }] }, finish_reason: null }] };
    res.write(`data: ${JSON.stringify(chunk)}\n\n`);
    res.write(`data: ${JSON.stringify({ ...chunk, choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] })}\n\n`);
    res.end("data: [DONE]\n\n");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const child = spawn(process.execPath, [fileURLToPath(new URL("../src/play.mjs", import.meta.url))], {
      env: { ...process.env, CCC_ALLOW_FIRST_LEGAL: "0", LLAMA_BASE_URL: `http://127.0.0.1:${server.address().port}` },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "", stderr = "";
    child.stdout.on("data", (c) => stdout += c);
    child.stderr.on("data", (c) => stderr += c);
    child.stdin.end(JSON.stringify({ board: "X23456789", turn: "O", legal: [2, 3, 4, 5, 6, 7, 8, 9] }));
    const code = await new Promise((resolve) => child.on("close", resolve));
    assert.equal(code, 0, stderr);
    assert.deepEqual(JSON.parse(stdout), { square: 3, source: "pi" });
    assert.equal(requests.length, 2);
    assert.equal(requests[0].path, "/v1/chat/completions");
    assert.deepEqual(requests[0].body.tools.map((t) => t.function.name).sort(), ["get_board", "submit_move"]);
    assert.match(JSON.stringify(requests[1].body.messages), /Illegal square 1/);
  } finally { await new Promise((resolve) => server.close(resolve)); }
});

test("Pi exits nonzero when the model answers without submitting", { skip: !hasPi, timeout: 20000 }, async () => {
  const server = createServer(async (req, res) => {
    for await (const _ of req) { /* drain request */ }
    res.writeHead(200, { "content-type": "text/event-stream" });
    const chunk = { id: "mock", object: "chat.completion.chunk", created: 1, model: "local-model",
      choices: [{ index: 0, delta: { role: "assistant", content: "I choose 3." }, finish_reason: null }] };
    res.write(`data: ${JSON.stringify(chunk)}\n\n`);
    res.write(`data: ${JSON.stringify({ ...chunk, choices: [{ index: 0, delta: {}, finish_reason: "stop" }] })}\n\n`);
    res.end("data: [DONE]\n\n");
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const child = spawn(process.execPath, [fileURLToPath(new URL("../src/play.mjs", import.meta.url))], {
      env: { ...process.env, CCC_ALLOW_FIRST_LEGAL: "0", LLAMA_BASE_URL: `http://127.0.0.1:${server.address().port}` },
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "", stderr = "";
    child.stdout.on("data", (c) => stdout += c);
    child.stderr.on("data", (c) => stderr += c);
    child.stdin.end(JSON.stringify({ board: "X23456789", turn: "O", legal: [2, 3, 4, 5, 6, 7, 8, 9] }));
    const code = await new Promise((resolve) => child.on("close", resolve));
    assert.notEqual(code, 0);
    assert.equal(stdout, "");
    assert.match(stderr, /did not submit a valid move/);
  } finally { await new Promise((resolve) => server.close(resolve)); }
});
