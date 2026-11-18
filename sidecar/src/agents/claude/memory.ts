// The user's own Claude Code auto memory of the workspace's first repo, read from its clone
// (docs/adr/claude-memory-per-repo.md): a session started in the workspace folder, not a repo, has
// a memory of its own and would never see what the user taught Claude in the clone. Read only:
// nothing here writes to it.

import { open, realpath } from "node:fs/promises";
import { join } from "node:path";
import { nativeHome } from "../context-capture.ts";
import { claudeProjectKey } from "./context-bundle.ts";

/**
 * What Claude Code loads of a `MEMORY.md`: its first lines, then its first characters, counted as
 * Claude Code counts them (a string's `length`; its docs round it to 25 KB).
 */
const INDEX_LINES = 200;
const INDEX_CHARS = 25_000;
/** Enough of a `MEMORY.md` for its head, whatever its size: a character is 4 bytes at most. */
const HEAD_BYTES = INDEX_CHARS * 4 + 4;

export interface CloneMemory {
  /** The clone as the daemon named it. */
  clone: string;
  /** The memory folder Claude Code keeps for the clone. */
  dir: string;
  /** The head of its `MEMORY.md`. */
  index: string;
  /** Whether `index` is only the head of the file. */
  cut: boolean;
}

/** The head of a `MEMORY.md`, cut as Claude Code cuts it when it loads one at session start. */
export function indexHead(text: string): { head: string; cut: boolean } {
  const trimmed = text.trim();
  const lines = trimmed.split("\n");
  if (lines.length <= INDEX_LINES && trimmed.length <= INDEX_CHARS) return { head: trimmed, cut: false };
  let head = lines.slice(0, INDEX_LINES).join("\n");
  if (head.length > INDEX_CHARS) {
    const end = head.lastIndexOf("\n", INDEX_CHARS);
    head = head.slice(0, end > 0 ? end : INDEX_CHARS);
  }
  return { head, cut: true };
}

/**
 * The memory Claude Code keeps for `clone` (its folder keyed as Claude Code keys the clone's real
 * path, under `CLAUDE_CONFIG_DIR` when `env` sets it); none without the clone or a non-empty
 * `MEMORY.md`.
 */
export async function readCloneMemory(clone: string, env: Record<string, string | undefined>): Promise<CloneMemory | undefined> {
  try {
    const key = await claudeProjectKey(await realpath(clone));
    const dir = join(nativeHome("claude", env), "projects", key, "memory");
    const { head, cut } = indexHead(await readHead(join(dir, "MEMORY.md")));
    return head ? { clone, dir, index: head, cut } : undefined;
  } catch {
    return undefined;
  }
}

async function readHead(path: string): Promise<string> {
  const file = await open(path);
  try {
    const buffer = Buffer.alloc(HEAD_BYTES);
    const { bytesRead } = await file.read(buffer, 0, HEAD_BYTES, 0);
    return buffer.toString("utf8", 0, bytesRead);
  } finally {
    await file.close();
  }
}

/** The memories a session is given: the first repo's, loaded, and the other repos', pointed at. */
export interface CloneMemories {
  first?: CloneMemory;
  others: CloneMemory[];
}

/** The memories of `clones` (the first one's loaded), read side by side; a clone without one left out. */
export async function readCloneMemories(clones: string[], env: Record<string, string | undefined>): Promise<CloneMemories> {
  const [first, ...others] = await Promise.all(clones.map((clone) => readCloneMemory(clone, env)));
  return { first, others: others.filter((memory) => memory !== undefined) };
}

/** What the session is told of the memory: whose it is, that it is read-only, and its index. */
export function memoryPrompt(memory: CloneMemory): string {
  const cut = memory.cut ? " (cut as Claude Code loads it: read the file for the rest)" : "";
  return [
    `The user's own Claude Code memory of the repo cloned at \`${memory.clone}\`: what they taught Claude while working in that clone, kept in \`${memory.dir}\`. Read-only: never write to that folder; what is worth remembering from this session goes to your own memory, where Claude Code keeps it. Its index, MEMORY.md, follows${cut}; read the other files of that folder when the work touches them.`,
    // As Claude Code wraps a memory: its text cannot close the element.
    `<user-memory path="${memory.dir}/MEMORY.md">\n${memory.index.replaceAll("<", "&lt;")}\n</user-memory>`,
  ].join("\n\n");
}

/** What the session is told of the other repos' memories: where they are, to read when needed. */
export function otherMemoriesPrompt(others: CloneMemory[]): string {
  return [
    "The user's own Claude Code memory of the workspace's other repos, read-only and not loaded: when the work touches one of them, read its folder's MEMORY.md, then the files it points to. Never write to these folders.",
    ...others.map((memory) => `- \`${memory.clone}\`: \`${memory.dir}\``),
  ].join("\n");
}
