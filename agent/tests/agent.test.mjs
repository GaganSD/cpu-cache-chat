import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { makeTools, parseBoard, llamaBaseUrl } from "../src/game.mjs";

const board = { board: "X23456789", turn: "O", legal: [2, 3, 4, 5, 6, 7, 8, 9] };
const Type = { Object: (o) => o, Integer: () => ({}) };

test("parse and validate board JSON", () => {
  assert.deepEqual(parseBoard(JSON.stringify(board)), board);
  for (const input of ["{", "null", JSON.stringify({ ...board, legal: [1] }),
    JSON.stringify({ ...board, legal: ["2"] }), JSON.stringify({ ...board, legal: [] })]) {
    assert.throws(() => parseBoard(input));
  }
});

test("illegal moves return feedback and do not choose a square", async () => {
  let stops = 0;
  const game = makeTools(board, Type, () => stops++);
  const submit = game.tools[1].execute;
  assert.match((await submit("id", { square: 1 })).content[0].text, /Illegal square 1.*2, 3/);
  assert.equal(game.square, null);
  assert.match((await submit("id", { square: 3 })).content[0].text, /Accepted 3/);
  assert.equal(game.square, 3);
  assert.equal(stops, 1);
  await submit("id", { square: 4 });
  assert.equal(game.square, 3);
  const invalid = makeTools(board, Type, () => stops++);
  for (let i = 0; i < 3; i++) await invalid.tools[1].execute("id", { square: 1 });
  assert.equal(invalid.square, null);
  assert.equal(stops, 2);
});

test("test hook runs without importing Pi", () => {
  const run = spawnSync(process.execPath, [new URL("./first-legal.mjs", import.meta.url).pathname], {
    input: JSON.stringify(board), encoding: "utf8",
  });
  assert.equal(run.status, 0, run.stderr);
  assert.deepEqual(JSON.parse(run.stdout), { square: 2, source: "test-hook" });
  const bad = spawnSync(process.execPath, [new URL("./first-legal.mjs", import.meta.url).pathname], {
    input: "broken", encoding: "utf8",
  });
  assert.notEqual(bad.status, 0);
  assert.equal(bad.stdout, "");
});

test("only literal HTTP loopback endpoints are accepted", () => {
  assert.equal(llamaBaseUrl("http://127.0.0.1:8081/v1"), "http://127.0.0.1:8081/v1");
  for (const url of ["http://localhost:8080", "https://127.0.0.1", "http://127.0.0.1.evil/", "http://127.0.0.1/?x=1"]) {
    assert.throws(() => llamaBaseUrl(url));
  }
});
