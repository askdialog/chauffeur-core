#!/usr/bin/env bun
// A stand-in for the agent sidecar, for the import from Claude Code. `context.copy` copies a
// session's transcript and folder from the project folder it names into the agents' Claude folder,
// under the project of `cwd`, never replacing a different file; as the real one, it refuses a
// `cwd` not named as the file system names it, and here an id starting with `unreadable`.
// `agent.session_messages` reads the copy's prompts and texts there (`CLAUDE_CONFIG_DIR`, as the
// real one), each at its time; it finds no copy of an id starting with `lost`. Anything else
// answers `{}`, but `agent.fork` its session id with `-fork`. Each request is kept, a line of
// `sidecar.log` in that folder.
import { appendFileSync, existsSync, mkdirSync, readdirSync, readFileSync, realpathSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { createInterface } from "node:readline";
const key = (cwd) => cwd.replace(/[^a-zA-Z0-9]/g, "-");
const walk = (dir) => readdirSync(dir).flatMap((name) => statSync(join(dir, name)).isDirectory() ? walk(join(dir, name)) : [join(dir, name)]);

function copy(p) {
  if (p.session_id.startsWith("unreadable")) throw new Error("The session could not be copied: unreadable");
  if (realpathSync(p.cwd) !== p.cwd) throw new Error("Native context files are unsafe or incomplete");
  const from = join(p.source_home, "projects", p.project_key);
  const files = [join(from, p.session_id + ".jsonl"), ...(existsSync(join(from, p.session_id)) ? walk(join(from, p.session_id)) : [])];
  const added = [];
  for (const path of files) {
    const to = join(p.claude_home, "projects", key(p.cwd), relative(from, path)), bytes = readFileSync(path);
    if (existsSync(to)) { if (!readFileSync(to).equals(bytes)) throw new Error("another file is there"); continue; }
    mkdirSync(dirname(to), { recursive: true });
    writeFileSync(to, bytes, { flag: "wx" });
    added.push(to);
  }
  return { files: added, folders: [] };
}

function messages(p) {
  const file = join(process.env.CLAUDE_CONFIG_DIR, "projects", key(p.cwd), p.session_id + ".jsonl");
  if (p.session_id.startsWith("lost") || !existsSync(file)) return { found: false, events: [] };
  const events = readFileSync(file, "utf8").trim().split("\n").map((l) => JSON.parse(l)).flatMap((row) =>
    row.type === "user" && typeof row.message?.content === "string" ? [{ at: row.timestamp, event: { type: "user_message", text: row.message.content, queued: false } }]
    : row.type === "assistant" && Array.isArray(row.message?.content) ? row.message.content.filter((b) => b.type === "text").map((b) => ({ at: row.timestamp, event: { type: "assistant_text", text: b.text, message_id: row.message.id + ":0" } }))
    : []);
  return { found: true, events };
}

for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line), p = request.params ?? {};
  if (process.env.CLAUDE_CONFIG_DIR) appendFileSync(join(process.env.CLAUDE_CONFIG_DIR, "sidecar.log"), line + "\n");
  if (request.id === undefined) continue;
  try {
    const result = request.method === "context.copy" ? copy(p) : request.method === "agent.session_messages" ? messages(p)
      : request.method === "agent.fork" ? { session_id: p.session_id + "-fork" } : {};
    console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
  } catch (error) {
    console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32000, message: String(error.message) } }));
  }
}
