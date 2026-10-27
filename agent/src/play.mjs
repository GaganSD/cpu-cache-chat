#!/usr/bin/env node
import { stdin } from "node:process";
import { makeTools, parseBoard, llamaBaseUrl } from "./game.mjs";

try {
  let raw = "";
  for await (const chunk of stdin) {
    raw += chunk;
    if (raw.length > 4096) throw new Error("board input too large");
  }
  const state = parseBoard(raw);
  {
    // A hard deadline covers SDK startup, inference, and abort cleanup.
    const deadline = setTimeout(() => {
      console.error("agent: timed out");
      process.exit(124);
    }, 90_000);
    deadline.unref();
    const baseUrl = llamaBaseUrl(process.env.LLAMA_BASE_URL);
    // Keep all SDK fetches on this exact loopback origin, including redirects.
    const localOrigin = new URL(baseUrl).origin;
    const localFetch = globalThis.fetch;
    globalThis.fetch = (input, init = {}) => {
      const target = new URL(typeof input === "string" || input instanceof URL ? input : input.url);
      if (target.origin !== localOrigin) throw new Error("refusing non-local model request");
      return localFetch(input, { ...init, redirect: "error" });
    };
    const { createAgentSession, createExtensionRuntime, ModelRuntime, SessionManager, SettingsManager } =
      await import("@earendil-works/pi-coding-agent");
    const { Type } = await import("typebox");
    const runtime = await ModelRuntime.create({ modelsPath: null, refreshOnCreate: false, allowModelNetwork: false });
    const modelId = process.env.LLAMA_MODEL || "local-model";
    runtime.registerProvider("ccc-local", {
      baseUrl, api: "openai-completions", apiKey: "local", authHeader: false,
      models: [{ id: modelId, name: modelId, reasoning: false, input: ["text"],
        cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 4096, maxTokens: 256,
        compat: { supportsStore: false, supportsDeveloperRole: false, supportsReasoningEffort: false,
          supportsUsageInStreaming: true, supportsStrictMode: false, maxTokensField: "max_tokens" } }],
    });
    const model = runtime.getModel("ccc-local", modelId);
    if (!model) throw new Error("local model registration failed");
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
    ({ session } = await createAgentSession({ model, modelRuntime: runtime, thinkingLevel: "off",
      tools: ["get_board", "submit_move"], customTools: game.tools, resourceLoader,
      sessionManager: SessionManager.inMemory(),
      settingsManager: SettingsManager.inMemory({ compaction: { enabled: false }, retry: { enabled: false } }),
    }));
    try {
      const active = session.getActiveToolNames().sort();
      if (active.join(",") !== "get_board,submit_move") throw new Error("unexpected active tools");
      await session.prompt(`Board: ${state.board}. Turn: ${state.turn}. Legal: ${state.legal.join(",")}. Submit one move.`);
    } finally {
      session.dispose();
      clearTimeout(deadline);
    }
    if (game.square === null) throw new Error("model did not submit a valid move");
    console.log(JSON.stringify({ square: game.square, source: "pi" }));
  }
} catch (err) {
  console.error(`agent: ${err?.message || err}`);
  process.exitCode = 1;
}
