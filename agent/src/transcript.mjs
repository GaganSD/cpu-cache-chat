import { constants, openSync, fstatSync, writeFileSync, closeSync } from "node:fs";

export const TRANSCRIPT_FILE_LIMIT = 16 * 1024 * 1024;

// Runs inside the independently supervised agent, never the physical wire owner.
export function appendTranscript(path, text, limit = TRANSCRIPT_FILE_LIMIT) {
  const fd = openSync(path, constants.O_WRONLY | constants.O_CREAT | constants.O_APPEND |
    constants.O_NOFOLLOW | constants.O_NONBLOCK, 0o600);
  try {
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.nlink !== 1 || (stat.mode & 0o077) !== 0 ||
        (process.getuid && stat.uid !== process.getuid())) {
      throw new Error("transcript target must be a private owned regular file");
    }
    if (stat.size + Buffer.byteLength(text) > limit) {
      throw new Error("transcript file quota exceeded; evidence incomplete");
    }
    writeFileSync(fd, text);
  } finally {
    closeSync(fd);
  }
}
