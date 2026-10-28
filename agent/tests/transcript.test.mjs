import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, statSync, chmodSync, symlinkSync, linkSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { appendTranscript } from "../src/transcript.mjs";

function fixture(run) {
  const dir = mkdtempSync(join(tmpdir(), "ccc-transcript-"));
  try { run(dir); } finally { rmSync(dir, { recursive: true, force: true }); }
}

test("transcripts append privately with a hard total budget", () => fixture(dir => {
  const file = join(dir, "agent.log");
  appendTranscript(file, "first\n", 12);
  appendTranscript(file, "next\n", 12);
  assert.equal(statSync(file).mode & 0o777, 0o600);
  assert.equal(readFileSync(file, "utf8"), "first\nnext\n");
  assert.throws(() => appendTranscript(file, "over quota\n", 12), /quota exceeded/);
  assert.equal(readFileSync(file, "utf8"), "first\nnext\n");
}));

test("transcript targets reject symlinks, hardlinks, and public modes", () => fixture(dir => {
  const file = join(dir, "real");
  appendTranscript(file, "original\n");
  const symlink = join(dir, "symlink");
  symlinkSync(file, symlink);
  assert.throws(() => appendTranscript(symlink, "bad"));
  const hardlink = join(dir, "hardlink");
  linkSync(file, hardlink);
  assert.throws(() => appendTranscript(hardlink, "bad"), /private owned regular/);
  rmSync(hardlink);
  chmodSync(file, 0o644);
  assert.throws(() => appendTranscript(file, "bad"), /private owned regular/);
  assert.equal(readFileSync(file, "utf8"), "original\n");
}));
