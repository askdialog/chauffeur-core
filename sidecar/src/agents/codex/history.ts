// The history a Codex thread sends the model, read from its rollout files: the app-server lists a
// thread's items as the app shows them, never the raw Responses API items (encrypted reasoning,
// tool outputs as the model got them) that Jev's compaction rebuilds the thread from.
//
// A rollout is one JSON object per line. A file may start from a prefix of another one
// (`session_meta.history_base`, after a fork or a revert); a `compacted` line replaces everything
// before it with its `replacement_history`. Anything else this reader does not know how to replay
// (a rollback, a compaction without its history) throws: the caller compacts the Codex way instead.

import { createReadStream } from "node:fs";
import { readdir } from "node:fs/promises";
import { basename, dirname, join } from "node:path";
import type { JsonValue } from "../../protocol.ts";
import { LineSplitter } from "../../util/lines.ts";

/** A Responses API item, as Codex keeps it. */
export type ResponseItem = { [key: string]: JsonValue } & { type: string };

const MAX_BASES = 32;
const SESSION_DIRS = ["sessions", "archived_sessions"];
const DAY_MS = 24 * 60 * 60 * 1000;

/**
 * The items the thread whose rollout is at `path` sends the model, oldest first. Read as a stream:
 * a rollout runs to megabytes, and the sidecar's other chats keep going meanwhile.
 */
export function readThreadHistory(path: string): Promise<ResponseItem[]> {
  return historyOf(path, undefined, 0);
}

async function historyOf(path: string, endByte: number | undefined, depth: number): Promise<ResponseItem[]> {
  if (depth > MAX_BASES) throw new Error(`${basename(path)} rests on more than ${MAX_BASES} earlier rollouts`);
  let items: ResponseItem[] = [];
  let first = true;
  const replay = async (line: string) => {
    if (!line.trim()) return;
    const entry: unknown = JSON.parse(line);
    if (!isObject(entry) || typeof entry.type !== "string") throw new Error(`${basename(path)} has a line Chauffeur does not know`);
    const payload = entry.payload;
    if (first && entry.type === "session_meta" && isObject(payload) && isObject(payload.history_base)) {
      const base = payload.history_base;
      if (typeof base.thread_id !== "string" || typeof base.end_byte_offset !== "number") {
        throw new Error(`${basename(path)} names its base history in a way Chauffeur does not know`);
      }
      items = await historyOf(await rolloutOf(path, base.thread_id), base.end_byte_offset, depth + 1);
    }
    first = false;
    if (entry.type === "response_item" && isItem(payload)) {
      items.push(payload);
    } else if (entry.type === "compacted") {
      const replacement = isObject(payload) ? payload.replacement_history : undefined;
      if (!Array.isArray(replacement) || !replacement.every(isItem)) {
        throw new Error(`${basename(path)} has a compaction without the history it left`);
      }
      items = [...replacement];
    } else if (entry.type === "event_msg" && isObject(payload) && payload.type === "thread_rolled_back") {
      throw new Error(`${basename(path)} has a rollback, which Chauffeur does not replay`);
    }
  };
  const splitter = new LineSplitter();
  // A stream's `end` is inclusive, and cannot ask for no bytes.
  const stream = endByte === undefined ? createReadStream(path) : endByte > 0 ? createReadStream(path, { end: endByte - 1 }) : [];
  for await (const chunk of stream as AsyncIterable<Buffer>) {
    const lines: string[] = [];
    splitter.push(chunk, (line) => lines.push(line));
    for (const line of lines) await replay(line);
  }
  await replay(splitter.end());
  return items;
}

/**
 * The rollout of the thread `id` (a base of the one at `path`): in the same tree of session
 * folders, named `…<id>.jsonl`.
 */
async function rolloutOf(path: string, id: string): Promise<string> {
  if (!/^[0-9a-f-]+$/i.test(id)) throw new Error(`${basename(path)} names an unexpected base thread`);
  let dir = dirname(path);
  while (!SESSION_DIRS.includes(basename(dir))) {
    const parent = dirname(dir);
    if (parent === dir) throw new Error(`${basename(path)} is not in Codex's sessions folder`);
    dir = parent;
  }
  const home = dirname(dir);
  const suffix = `${id}.jsonl`;
  for (const day of creationDays(id)) {
    const found = await findFile(join(home, "sessions", day), suffix);
    if (found) return found;
  }
  for (const name of SESSION_DIRS) {
    const found = await findFile(join(home, name), suffix);
    if (found) return found;
  }
  throw new Error(`the rollout of thread ${id}, which ${basename(path)} rests on, is missing`);
}

/**
 * The `YYYY/MM/DD` folders Codex may have filed a thread under, from the creation time a version 7
 * id carries: the day in local time, and the days on each side for a rollout written in another
 * time zone. None for another id: the caller walks every folder.
 */
function creationDays(id: string): string[] {
  const hex = id.replaceAll("-", "");
  if (hex.length !== 32 || hex[12] !== "7") return [];
  const created = Number.parseInt(hex.slice(0, 12), 16);
  return [0, -DAY_MS, DAY_MS].map((offset) => {
    const day = new Date(created + offset);
    const pad = (n: number) => String(n).padStart(2, "0");
    return join(String(day.getFullYear()), pad(day.getMonth() + 1), pad(day.getDate()));
  });
}

async function findFile(dir: string, suffix: string): Promise<string | undefined> {
  let entries;
  try {
    entries = await readdir(dir, { withFileTypes: true });
  } catch {
    return undefined;
  }
  for (const entry of entries) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = await findFile(path, suffix);
      if (found) return found;
    } else if (entry.name.endsWith(suffix)) {
      return path;
    }
  }
  return undefined;
}

function isObject(value: unknown): value is { [key: string]: JsonValue } {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isItem(value: unknown): value is ResponseItem {
  return isObject(value) && typeof value.type === "string";
}
