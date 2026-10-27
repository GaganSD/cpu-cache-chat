#!/usr/bin/env node
// Test fixture only. Not shipped as the demo player.
import { parseBoard } from "../src/game.mjs";

let raw = "";
for await (const chunk of process.stdin) raw += chunk;
const state = parseBoard(raw);
process.stdout.write(JSON.stringify({ square: state.legal[0], source: "test-hook" }) + "\n");
