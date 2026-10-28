import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { llamaBaseUrl, OPENCODE_GO_MODELS } from "../src/game.mjs";

const board = JSON.stringify({ board: "123456789", turn: "X", legal: [1,2,3,4,5,6,7,8,9] });
async function invoke(url) {
  const child = spawn(process.execPath, [fileURLToPath(new URL("../src/play.mjs", import.meta.url))], {
    env: { ...process.env, CCC_AGENT_PROVIDER: "llama", LLAMA_BASE_URL: url }, stdio: ["pipe", "pipe", "pipe"],
  });
  let stdout = "", stderr = "";
  child.stdout.on("data", c => stdout += c); child.stderr.on("data", c => stderr += c);
  child.stdin.end(board);
  const code = await new Promise(resolve => child.on("close", resolve));
  return { code, stdout, stderr };
}
async function listen(server) { await new Promise(resolve => server.listen(0, "127.0.0.1", resolve)); return `http://127.0.0.1:${server.address().port}`; }

test("local URL requires literal IPv4 spelling", () => {
  for (const url of ["http://127.1", "http://2130706433", "http://0x7f000001", "http://127.0.0.1:80@evil"]) assert.throws(() => llamaBaseUrl(url));
});
test("provider redirects cannot reach another origin", { timeout: 20000 }, async () => {
  let reached = false;
  const target = createServer((_req, res) => { reached = true; res.end(); });
  const targetUrl = await listen(target);
  const source = createServer((_req, res) => { res.writeHead(302, { location: targetUrl }); res.end(); });
  const sourceUrl = await listen(source);
  try { const result = await invoke(sourceUrl); assert.notEqual(result.code, 0); assert.equal(result.stdout, ""); assert.equal(reached, false); }
  finally { await Promise.all([new Promise(r => source.close(r)), new Promise(r => target.close(r))]); }
});
test("oversized model transcript fails explicitly", { timeout: 20000 }, async () => {
  const server = createServer(async (req, res) => {
    for await (const _ of req) { /* consume */ }
    res.writeHead(200, { "content-type": "text/event-stream" });
    res.end("x".repeat(1100000));
  });
  const url = await listen(server);
  try { const result = await invoke(url); assert.notEqual(result.code, 0); assert.equal(result.stdout, ""); assert.match(result.stderr, /quota exceeded/); }
  finally { await new Promise(r => server.close(r)); }
});

test("hosted model allowlist is limited to approved deepseek with low reasoning", () => {
  assert.deepEqual(Object.keys(OPENCODE_GO_MODELS), ["deepseek-v4.1-flash"]);
  assert.equal(OPENCODE_GO_MODELS["deepseek-v4.1-flash"].thinkingLevel, "low");
});
test("gateway rejects non-allowlisted model without network", async () => {
  const child = spawn(process.execPath, [fileURLToPath(new URL("../src/play.mjs", import.meta.url))], {
    env: { ...process.env, CCC_AGENT_PROVIDER: "vercel-ai-gateway", CCC_AGENT_MODEL: "not-approved", AI_GATEWAY_API_KEY: "fake-secret" }, stdio: ["pipe", "pipe", "pipe"],
  });
  let stderr = ""; child.stderr.on("data", c => stderr += c); child.stdin.end(board);
  const code = await new Promise(resolve => child.on("close", resolve));
  assert.notEqual(code, 0); assert.match(stderr, /unsupported/); assert.doesNotMatch(stderr, /fake-secret/);
});
