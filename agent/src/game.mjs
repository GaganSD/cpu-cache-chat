export function parseBoard(raw) {
  let state;
  try { state = JSON.parse(raw); } catch { throw new Error("expected JSON board on stdin"); }
  if (!state || typeof state !== "object" || typeof state.board !== "string" ||
      !/^[XO1-9]{9}$/.test(state.board) || !["X", "O"].includes(state.turn) ||
      !Array.isArray(state.legal) || state.legal.some((n) => !Number.isInteger(n) || n < 1 || n > 9) ||
      new Set(state.legal).size !== state.legal.length ||
      [...state.board].some((c, i) => c !== "X" && c !== "O" && c !== String(i + 1)) ||
      state.legal.some((n) => state.board[n - 1] !== String(n))) {
    throw new Error("invalid board, turn, or legal squares");
  }
  if (!state.legal.length) throw new Error("no legal moves");
  return { board: state.board, turn: state.turn, legal: state.legal };
}

export function makeTools(state, Type, stop) {
  let square = null;
  let attempts = 0;
  const result = (text) => ({ content: [{ type: "text", text }], details: {} });
  return {
    get square() { return square; },
    tools: [
      {
        name: "get_board", label: "Get board", description: "Read the board and legal squares.",
        parameters: Type.Object({}),
        execute: async () => result(JSON.stringify(state)),
      },
      {
        name: "submit_move", label: "Submit move", description: "Submit one legal square.",
        parameters: Type.Object({ square: Type.Integer() }),
        execute: async (_id, params) => {
          if (square !== null) return result(`Move ${square} already accepted.`);
          attempts++;
          if (!state.legal.includes(params.square)) {
            if (attempts >= 3) stop();
            return result(`Illegal square ${params.square}. Legal squares: ${state.legal.join(", ")}.`);
          }
          square = params.square;
          stop();
          return result(`Accepted ${square}.`);
        },
      },
    ],
  };
}

// Narrow allowlist of OpenCode Go hosted models for CCC_AGENT_PROVIDER=opencode-go.
// Fields mirror the pi-ai provider registry so tool calling behaves the same;
// thinkingLevel is chosen per model because not every model supports "off".
export const OPENCODE_GO_MODELS = {
  "glm-5.3-flash": {
    reasoning: true,
    thinkingLevel: "off",
    compat: { supportsStore: false, supportsDeveloperRole: false, supportsStrictMode: true, maxTokensField: "max_tokens" },
  },
  "deepseek-v4.1-flash": {
    reasoning: true,
    thinkingLevel: "low",
    compat: { supportsStore: false, supportsDeveloperRole: false, supportsStrictMode: true, maxTokensField: "max_tokens",
      requiresReasoningContentOnAssistantMessages: true, thinkingFormat: "deepseek" },
  },
};

export function llamaBaseUrl(raw = "http://127.0.0.1:8080") {
  let url;
  try { url = new URL(raw); } catch { throw new Error("invalid LLAMA_BASE_URL"); }
  if (!/^http:\/\/127\.0\.0\.1(?::[0-9]+)?(?:\/|$)/.test(raw) ||
      url.protocol !== "http:" || url.hostname !== "127.0.0.1" || url.username || url.password ||
      url.search || url.hash || !["/", "/v1", "/v1/"].includes(url.pathname)) {
    throw new Error("LLAMA_BASE_URL must be http://127.0.0.1[:port], optionally ending in /v1");
  }
  return `${url.origin}/v1`;
}
