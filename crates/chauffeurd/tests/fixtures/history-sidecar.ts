#!/usr/bin/env bun
// A stand-in for the agent sidecar, for the import from Claude Code: `context.capture` bundles a
// session's files from the project folder it names (blocking ids starting with `unreadable`),
// `context.restore` adds them to the agents' Claude folder under the project of `cwd`, never
// replacing a different file, and `agent.session_messages` reads the copy's prompts and texts
// there (`CLAUDE_CONFIG_DIR`, as the real one), each at its time. Anything else answers `{}`.
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, relative } from "node:path";
import { createInterface } from "node:readline";
const key = (cwd) => cwd.replace(/[^a-zA-Z0-9]/g, "-");
const walk = (dir) => readdirSync(dir).flatMap((name) => statSync(join(dir, name)).isDirectory() ? walk(join(dir, name)) : [join(dir, name)]);
for await (const line of createInterface({ input: process.stdin })) {
  const request = JSON.parse(line), p = request.params ?? {};
  if (request.id === undefined) continue;
  let result = {};
  if (request.method === "context.capture") {
    const dir = join(p.env.CLAUDE_CONFIG_DIR, "projects", p.project_key);
    if (p.session_id.startsWith("unreadable")) result = { outcome: "blocked", reason: "Claude context is incomplete or unsupported" };
    else {
      const files = [join(dir, p.session_id + ".jsonl"), ...(existsSync(join(dir, p.session_id)) ? walk(join(dir, p.session_id)) : [])]
        .map((path) => ({ path: relative(dir, path), content: readFileSync(path).toString("base64") }));
      const bytes = Buffer.from(JSON.stringify({ sessionId: p.session_id, files }));
      writeFileSync(p.output, bytes, { flag: "wx", mode: 0o600 });
      result = { outcome: "captured", sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length };
    }
  } else if (request.method === "context.restore") {
    const added = [];
    for (const entry of p.contexts) {
      const bundle = JSON.parse(readFileSync(entry.path, "utf8"));
      for (const file of bundle.files) {
        const to = join(p.claude_home, "projects", key(entry.cwd), file.path), bytes = Buffer.from(file.content, "base64");
        if (existsSync(to)) { if (!readFileSync(to).equals(bytes)) throw new Error("a different file is there"); continue; }
        mkdirSync(dirname(to), { recursive: true });
        writeFileSync(to, bytes, { flag: "wx" });
        added.push(to);
      }
    }
    result = { files: added, folders: [] };
  } else if (request.method === "agent.session_messages") {
    const file = join(process.env.CLAUDE_CONFIG_DIR, "projects", key(p.cwd), p.session_id + ".jsonl");
    const events = existsSync(file) ? readFileSync(file, "utf8").trim().split("\n").map((l) => JSON.parse(l)).flatMap((row) =>
      row.type === "user" && typeof row.message?.content === "string" ? [{ at: row.timestamp, event: { type: "user_message", text: row.message.content, queued: false } }]
      : row.type === "assistant" && Array.isArray(row.message?.content) ? row.message.content.filter((b) => b.type === "text").map((b) => ({ at: row.timestamp, event: { type: "assistant_text", text: b.text, message_id: row.message.id + ":0" } }))
      : []) : [];
    result = { found: existsSync(file), events };
  }
  console.log(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
}
