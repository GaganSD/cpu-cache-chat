#!/usr/bin/env node
// Narrow player. Prefers Pi SDK when available; otherwise refuses to invent a minimax.

import { stdin } from "node:process";

const raw = await readStdin();
let state;
try {
  state = JSON.parse(raw.trim());
} catch {
  console.error("agent: expected JSON board on stdin");
  process.exit(2);
}

const legal = Array.isArray(state.legal) ? state.legal.map(Number) : [];
if (legal.length === 0) {
  console.error("agent: no legal moves");
  process.exit(3);
}

if (process.env.CCC_ALLOW_FIRST_LEGAL === "1") {
  // Explicit test hook only. Never the demo player.
  console.log(JSON.stringify({ square: legal[0], source: "test-hook" }));
  process.exit(0);
}

try {
  const { createAgentSession, SessionManager } = await import("@earendil-works/pi-coding-agent");
  const chosen = { square: null };
  const { session } = await createAgentSession({
    sessionManager: SessionManager.inMemory(),
    noTools: true,
    customTools: [
      {
        name: "get_board",
        label: "get_board",
        description: "Read the local tic-tac-toe board",
        parameters: { type: "object", properties: {} },
        execute: async () => JSON.stringify(state),
      },
      {
        name: "submit_move",
        label: "submit_move",
        description: "Submit one legal square 1-9",
        parameters: {
          type: "object",
          properties: { square: { type: "integer", minimum: 1, maximum: 9 } },
          required: ["square"],
        },
        execute: async (toolCall) => {
          const square = Number(toolCall?.arguments?.square);
          if (!legal.includes(square)) {
            return `illegal square ${square}. legal: ${legal.join(",")}`;
          }
          chosen.square = square;
          return `accepted ${square}`;
        },
      },
    ],
  });
  try {
    await session.prompt(
      `You are a tic-tac-toe player. Mark=${state.turn}. Board compact=${state.board}. ` +
        `Call get_board if needed, then submit_move with one legal square from ${legal.join(",")}. ` +
        `Do not claim a win you have not submitted. No other tools.`,
    );
  } finally {
    session.dispose();
  }
  if (!chosen.square) {
    console.error("agent: model did not submit a move");
    process.exit(4);
  }
  console.log(JSON.stringify({ square: chosen.square, source: "pi" }));
} catch (err) {
  console.error(`agent: Pi SDK unavailable or failed: ${err?.message || err}`);
  process.exit(5);
}

function readStdin() {
  return new Promise((resolve, reject) => {
    let data = "";
    stdin.setEncoding("utf8");
    stdin.on("data", (c) => {
      data += c;
    });
    stdin.on("end", () => resolve(data));
    stdin.on("error", reject);
  });
}
