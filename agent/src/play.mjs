#!/usr/bin/env node
import { stdin } from "node:process";
import { makeTools, parseBoard, llamaBaseUrl, OPENCODE_GO_MODELS } from "./game.mjs";

// Covers input, SDK startup, inference, and cleanup.
const deadline = setTimeout(() => {
  console.error("agent: timed out");
  process.exit(124);
}, 90_000);
deadline.unref();
try {
  let raw = "";
  for await (const chunk of stdin) {
    raw += chunk;
    if (raw.length > 4096) throw new Error("board input too large");
  }
  const state = parseBoard(raw);
  {
    // Provider selection. Default is a container-local llama.cpp server on
    // loopback. "opencode-go" uses the hosted OpenCode Go inference API with a
    // key supplied via the environment; the container then needs network
    // egress to that one origin, but never a route to the peer container.
    const providerName = process.env.CCC_AGENT_PROVIDER || "llama";
    let baseUrl, providerId, modelId, registration, spec;
    if (providerName === "opencode-go") {
      const apiKey = process.env.OPENCODE_GO_API_KEY;
      if (!apiKey) throw new Error("OPENCODE_GO_API_KEY is required for CCC_AGENT_PROVIDER=opencode-go");
      baseUrl = "https://opencode.ai/inference/go/v1";
      providerId = "opencode-go";
      modelId = process.env.CCC_AGENT_MODEL || "glm-5.3-flash";
      spec = OPENCODE_GO_MODELS[modelId];
      if (!spec) throw new Error(`unsupported opencode-go model ${modelId}; allowed: ${Object.keys(OPENCODE_GO_MODELS).join(", ")}`);
      registration = {
        baseUrl, api: "openai-completions", apiKey, authHeader: true,
        models: [{ id: modelId, name: modelId, reasoning: spec.reasoning, input: ["text"],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
          contextWindow: 8192, maxTokens: 2048, compat: spec.compat }],
      };
    } else if (providerName === "llama") {
      baseUrl = llamaBaseUrl(process.env.LLAMA_BASE_URL);
      providerId = "ccc-local";
      modelId = process.env.LLAMA_MODEL || "local-model";
      registration = {
        baseUrl, api: "openai-completions", apiKey: "local", authHeader: false,
        models: [{ id: modelId, name: modelId, reasoning: false, input: ["text"],
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 4096, maxTokens: 256,
          compat: { supportsStore: false, supportsDeveloperRole: false, supportsReasoningEffort: false,
            supportsUsageInStreaming: true, supportsStrictMode: false, maxTokensField: "max_tokens" } }],
      };
    } else {
      throw new Error(`unknown CCC_AGENT_PROVIDER ${providerName}`);
    }
    // Keep all SDK fetches on this exact provider origin, including redirects.
    const allowedOrigin = new URL(baseUrl).origin;
    const realFetch = globalThis.fetch;
    let transcriptBytes = 0;
    let transcriptExceeded = false;
    const transcriptLimit = 1024 * 1024;
    globalThis.fetch = async (input, init = {}) => {
      const target = new URL(typeof input === "string" || input instanceof URL ? input : input.url);
      if (target.origin !== allowedOrigin) throw new Error("refusing off-provider model request");
      const response = await realFetch(input, { ...init, redirect: "error" });
      if (!response.body) return response;
      const body = response.body.pipeThrough(new TransformStream({
        transform(chunk, controller) {
          transcriptBytes += chunk.byteLength;
          if (transcriptBytes > transcriptLimit) {
            transcriptExceeded = true;
            controller.error(new Error("agent transcript quota exceeded; evidence incomplete"));
          } else controller.enqueue(chunk);
        },
      }));
      return new Response(body, { status: response.status, statusText: response.statusText, headers: response.headers });
    };
    const { createAgentSession, createExtensionRuntime, ModelRuntime, SessionManager, SettingsManager } =
      await import("@earendil-works/pi-coding-agent");
    const { Type } = await import("typebox");
    const runtime = await ModelRuntime.create({ modelsPath: null, refreshOnCreate: false, allowModelNetwork: false });
    runtime.registerProvider(providerId, registration);
    const model = runtime.getModel(providerId, modelId);
    if (!model) throw new Error("model registration failed");
    // No discovered extensions, files, skills, prompts, settings, or session persistence.
    const resourceLoader = {
      getExtensions: () => ({ extensions: [], errors: [], runtime: createExtensionRuntime() }),
      getSkills: () => ({ skills: [], diagnostics: [] }),
      getPrompts: () => ({ prompts: [], diagnostics: [] }),
      getThemes: () => ({ themes: [], diagnostics: [] }),
      getAgentsFiles: () => ({ agentsFiles: [] }),
      getSystemPrompt: () => "Play tic-tac-toe. Submit one legal square using submit_move.",
      getSystemPromptSource: () => undefined, getAppendSystemPrompt: () => [],
      getAppendSystemPromptSources: () => [], extendResources: () => {}, reload: async () => {},
    };
    let session;
    const game = makeTools(state, Type, () => setImmediate(() => { void session?.abort(); }));
    ({ session } = await createAgentSession({ model, modelRuntime: runtime,
      thinkingLevel: providerName === "opencode-go" ? spec.thinkingLevel : "off",
      tools: ["get_board", "submit_move"], customTools: game.tools, resourceLoader,
      sessionManager: SessionManager.inMemory(),
      settingsManager: SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } }),
    }));
    try {
      const active = session.getActiveToolNames().sort();
      if (active.join(",") !== "get_board,submit_move") throw new Error("unexpected active tools");
      await session.prompt(`Board: ${state.board}. Turn: ${state.turn}. Legal: ${state.legal.join(",")}. Submit one move.`);
      if (transcriptExceeded) throw new Error("agent transcript quota exceeded; evidence incomplete");
      if (game.square === null) {
        // One bounded corrective reprompt; a second silence is a visible failure.
        await session.prompt("No move was submitted. Call submit_move exactly once with one of the legal squares.");
      }
      if (transcriptExceeded) throw new Error("agent transcript quota exceeded; evidence incomplete");
    } finally {
      if (process.env.CCC_AGENT_DEBUG) {
        try {
          const messages = session.messages ?? [];
          let bytes = 0;
          for (let i = 0; i < messages.length; i++) {
            const line = `agent-debug message ${i}: ${JSON.stringify(messages[i])}`;
            bytes += Buffer.byteLength(line) + 1;
            if (bytes > transcriptLimit) {
              transcriptExceeded = true;
              console.error("agent transcript quota exceeded; evidence incomplete");
              break;
            }
            console.error(line);
          }
        } catch (e) { console.error(`agent-debug failed: ${e?.message || e}`); }
      }
      session.dispose();
    }
    if (transcriptExceeded) throw new Error("agent transcript quota exceeded; evidence incomplete");
    if (game.square === null) throw new Error("model did not submit a valid move");
    console.log(JSON.stringify({ square: game.square, source: "pi" }));
  }
} catch (err) {
  console.error(`agent: ${err?.message || err}`);
  process.exitCode = 1;
} finally {
  clearTimeout(deadline);
}
