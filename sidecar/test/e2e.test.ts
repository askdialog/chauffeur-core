// Smoke test of the compiled sidecar against real Claude sessions. Opt-in:
//
//   bun run build && CHAUFFEUR_SIDECAR_E2E=1 bun test test/e2e.test.ts
//
// Needs a Claude Code login on the machine. Spawns dist/chauffeur-sidecar (or
// CHAUFFEUR_SIDECAR_BIN) without CHAUFFEUR_CLAUDE_PATH, so the embedded Claude binary is used.
// CHAUFFEUR_SIDECAR_E2E_LOG=<file> keeps the sidecar's stderr.

import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { existsSync, mkdirSync, mkdtempSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { ChatEvent } from "../src/protocol.ts";
import { CLAUDE_AGENT_SDK_VERSION } from "../src/version.ts";

const enabled = process.env.CHAUFFEUR_SIDECAR_E2E === "1";
const binary = process.env.CHAUFFEUR_SIDECAR_BIN ?? join(import.meta.dir, "..", "dist", "chauffeur-sidecar");
const MODEL = "claude-haiku-4-5-20251001";
/** The cheapest Codex model `model/list` offers this account ("Fast and affordable"). */
const CODEX_MODEL = process.env.CHAUFFEUR_E2E_CODEX_MODEL ?? "gpt-6-luna";
const codexEnabled = process.env.CHAUFFEUR_SIDECAR_E2E_CODEX !== "0";
const TURN_TIMEOUT_MS = 120_000;
/** A stdio MCP server with one tool, named after its first argument. */
const FAKE_MCP = `import json, sys
name = sys.argv[1]
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    if msg.get("method") == "initialize":
        result = {"protocolVersion": msg["params"].get("protocolVersion", "2025-06-18"), "capabilities": {"tools": {}}, "serverInfo": {"name": name, "version": "1"}}
    elif msg.get("method") == "tools/list":
        result = {"tools": [{"name": name + "_ping", "description": "ping", "inputSchema": {"type": "object", "properties": {}}}]}
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": result}), flush=True)
`;

type Message = {
  id?: number | string | null;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { code: number; message: string };
};

/** The daemon's side of the pipe: requests, notifications, and answers to the sidecar's hooks. */
class FakeDaemon {
  readonly events: Array<{ chat: string; event: ChatEvent }> = [];
  readonly hookRequests: Message[] = [];
  stderr = "";
  private readonly proc: Bun.Subprocess<"pipe", "pipe", "pipe">;
  private readonly waiting = new Map<number, (message: Message) => void>();
  private listeners: Array<() => void> = [];
  private nextId = 1;

  constructor(cwd: string, overrides: Record<string, string> = {}) {
    const env: Record<string, string | undefined> = { ...process.env, CHAUFFEUR_SIDECAR_LOG: "debug" };
    delete env.CHAUFFEUR_CLAUDE_PATH;
    Object.assign(env, overrides);
    this.proc = Bun.spawn([binary], { cwd, env, stdin: "pipe", stdout: "pipe", stderr: "pipe" });
    void this.read();
    void this.readStderr();
  }

  request(method: string, params: unknown): Promise<Message> {
    const id = this.nextId++;
    return new Promise((resolve) => {
      this.waiting.set(id, resolve);
      this.write({ jsonrpc: "2.0", id, method, params });
    });
  }

  eventsOf(chat: string, from = 0): ChatEvent[] {
    return this.events.filter((e) => e.chat === chat).slice(from).map((e) => e.event);
  }

  /** Resolves with the first event of `chat` (from index `from`) matching, within `timeoutMs`. */
  async waitFor(chat: string, predicate: (event: ChatEvent) => boolean, from = 0, timeoutMs = TURN_TIMEOUT_MS): Promise<ChatEvent> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      const found = this.eventsOf(chat, from).find(predicate);
      if (found) return found;
      const left = deadline - Date.now();
      if (left <= 0) throw new Error(`timed out; events of ${chat}: ${JSON.stringify(this.eventsOf(chat, from), null, 1)}`);
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, left);
        this.listeners.push(() => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
  }

  async close(): Promise<number> {
    this.proc.stdin.end();
    return await this.proc.exited;
  }

  private write(message: unknown): void {
    this.proc.stdin.write(`${JSON.stringify(message)}\n`);
    this.proc.stdin.flush();
  }

  private async read(): Promise<void> {
    const decoder = new TextDecoder();
    let buffer = "";
    for await (const chunk of this.proc.stdout) {
      buffer += decoder.decode(chunk, { stream: true });
      let newline = buffer.indexOf("\n");
      while (newline >= 0) {
        this.receive(JSON.parse(buffer.slice(0, newline)) as Message);
        buffer = buffer.slice(newline + 1);
        newline = buffer.indexOf("\n");
      }
    }
  }

  private async readStderr(): Promise<void> {
    const decoder = new TextDecoder();
    for await (const chunk of this.proc.stderr) this.stderr += decoder.decode(chunk, { stream: true });
  }

  private receive(message: Message): void {
    if (message.method === "session.event") {
      const params = message.params as { chat_id: string; event: ChatEvent };
      this.events.push({ chat: params.chat_id, event: params.event });
      const listeners = this.listeners;
      this.listeners = [];
      for (const listener of listeners) listener();
    } else if (message.method !== undefined && message.id !== undefined) {
      // A hook request: these tests expect none, refuse it.
      this.hookRequests.push(message);
      this.write({ jsonrpc: "2.0", id: message.id, error: { code: 400, message: "not in this test" } });
    } else if (typeof message.id === "number") {
      this.waiting.get(message.id)?.(message);
      this.waiting.delete(message.id);
    }
  }
}

describe.skipIf(!enabled)("compiled sidecar with real agent sessions", () => {
  let repo: string;
  let daemon: FakeDaemon;

  const start = (chat: string, permission_mode: string, extra: Record<string, unknown> = {}) =>
    daemon.request("session.start", {
      chat_id: chat,
      workspace_id: "w-e2e",
      agent: "claude",
      cwd: repo,
      additional_directories: [],
      model: MODEL,
      permission_mode,
      env: { CHAUFFEUR_WORKSPACE_ID: "w-e2e" },
      ...extra,
    });
  const startCodex = (chat: string, permission_mode: string, extra: Record<string, unknown> = {}) =>
    start(chat, permission_mode, { agent: "codex", model: CODEX_MODEL, effort: "low", ...extra });

  beforeAll(() => {
    if (!existsSync(binary)) throw new Error(`${binary} is missing: run \`bun run build\` first`);
    repo = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-e2e-")));
    writeFileSync(join(repo, "README.md"), "# e2e\n");
    const commit = ["-c", "user.name=e2e", "-c", "user.email=e2e@example.com", "commit", "-qm", "init"];
    for (const args of [["init", "-q"], ["add", "."], commit]) {
      const git = Bun.spawnSync(["git", ...args], { cwd: repo });
      if (git.exitCode !== 0) throw new Error(`git ${args.join(" ")} failed: ${git.stderr.toString()}`);
    }
    daemon = new FakeDaemon(repo);
  });

  afterAll(async () => {
    const code = await daemon?.close();
    if (process.env.CHAUFFEUR_SIDECAR_E2E_LOG) writeFileSync(process.env.CHAUFFEUR_SIDECAR_E2E_LOG, daemon.stderr);
    if (repo) {
      rmSync(repo, { recursive: true, force: true });
      rmSync(`${repo}-wt`, { recursive: true, force: true });
      rmSync(`${repo}-cwt`, { recursive: true, force: true });
    }
    // On stdin EOF the sidecar stops every session and exits 0.
    expect(code).toBe(0);
  });

  test.skipIf(!codexEnabled)("names work with Luna when Claude is unavailable", async () => {
    const naming = new FakeDaemon(repo, { CHAUFFEUR_CLAUDE_PATH: join(repo, "missing-claude") });
    try {
      const response = await naming.request("agent.title", { text: "Fix checkout when a cart is empty after login" });
      expect(response.error).toBeUndefined();
      expect(response.result).toMatchObject({ title: expect.stringMatching(/checkout|cart|login/i) });
      expect(naming.events).toEqual([]);
    } finally {
      expect(await naming.close()).toBe(0);
    }
  }, 45_000);

  test(
    "starts, answers pong, and ends the turn successfully",
    async () => {
      const ping = await daemon.request("sidecar.ping", {});
      expect(ping.result).toMatchObject({ agents: { "claude-agent-sdk": CLAUDE_AGENT_SDK_VERSION } });

      const started = await start("pong", "bypass_permissions");
      expect(started.error).toBeUndefined();
      expect(started.result).toMatchObject({ agent_session_id: null });
      const send = await daemon.request("session.send", { chat_id: "pong", text: "Reply with the single word pong" });
      expect(send.result).toEqual({});

      const ended = await daemon.waitFor("pong", (e) => e.type === "turn_ended");
      const events = daemon.eventsOf("pong");
      const session = events.find((e) => e.type === "session_started");
      expect(session).toMatchObject({ type: "session_started", model: MODEL });
      expect(session?.type === "session_started" && session.agent_session_id.length).toBeGreaterThan(0);
      expect(events[0]).toEqual({ type: "turn_started" });
      const texts = events.filter((e) => e.type === "assistant_text");
      const pong = texts.find((e) => e.type === "assistant_text" && /pong/i.test(e.text));
      expect(pong).toBeDefined();
      expect(ended).toMatchObject({ type: "turn_ended", outcome: "success", error: null });
      if (ended.type === "turn_ended") {
        expect(ended.cost_usd).toBeGreaterThan(0);
        expect(ended.duration_ms).toBeGreaterThan(0);
        expect(ended.context_tokens).toBeGreaterThan(0);
      }
      // Streamed deltas carry the final block's message id.
      const deltas = events.filter((e) => e.type === "text_delta");
      if (deltas.length > 0 && pong?.type === "assistant_text") {
        expect(deltas.some((d) => d.type === "text_delta" && d.message_id === pong.message_id)).toBe(true);
      }
    },
    TURN_TIMEOUT_MS + 60_000,
  );

  test(
    "denies git worktree add even when permissions are bypassed",
    async () => {
      const from = daemon.eventsOf("pong").length;
      await daemon.request("session.send", {
        chat_id: "pong",
        text: `Use the Bash tool to run exactly this command, once, then stop: git worktree add ${repo}-wt -b e2e-wt`,
      });
      await daemon.waitFor("pong", (e) => e.type === "turn_ended", from);
      const events = daemon.eventsOf("pong", from);
      const call = events.find((e) => e.type === "tool_call" && e.name === "Bash");
      expect(call).toBeDefined();
      const result = events.find(
        (e) => e.type === "tool_result" && call?.type === "tool_call" && e.tool_use_id === call.tool_use_id,
      );
      expect(result).toMatchObject({ is_error: true });
      expect(result?.type === "tool_result" && result.output).toContain("chauffeur worktree add");
      expect(existsSync(`${repo}-wt`)).toBe(false);
      expect((await daemon.request("session.stop", { chat_id: "pong" })).result).toEqual({});
    },
    TURN_TIMEOUT_MS + 60_000,
  );

  test(
    "routes a permission request and a question to the daemon and back",
    async () => {
      expect((await start("ask", "default")).result).toMatchObject({ agent_session_id: null });

      await daemon.request("session.send", {
        chat_id: "ask",
        text: "Use the Bash tool to run exactly: touch e2e-permission.txt — then reply done.",
      });
      const permission = await daemon.waitFor("ask", (e) => e.type === "permission_request");
      if (permission.type !== "permission_request") throw new Error("unreachable");
      expect(permission.tool_name).toBe("Bash");
      const allowed = await daemon.request("session.respond", {
        chat_id: "ask",
        request_id: permission.request_id,
        response: { type: "allow", always: false },
      });
      expect(allowed.result).toEqual({});
      const firstTurn = await daemon.waitFor("ask", (e) => e.type === "turn_ended");
      expect(firstTurn).toMatchObject({ outcome: "success" });
      expect(daemon.eventsOf("ask")).toContainEqual({ type: "request_resolved", request_id: permission.request_id, outcome: "allowed" });
      expect(existsSync(join(repo, "e2e-permission.txt"))).toBe(true);

      const from = daemon.eventsOf("ask").length;
      await daemon.request("session.send", {
        chat_id: "ask",
        text: "Use the AskUserQuestion tool to ask me which color I prefer, red or blue. Then reply with my answer in one word.",
      });
      const question = await daemon.waitFor("ask", (e) => e.type === "question", from);
      if (question.type !== "question") throw new Error("unreachable");
      expect(question.questions.length).toBeGreaterThan(0);
      const asked = question.questions[0]?.question as string;
      const answered = await daemon.request("session.respond", {
        chat_id: "ask",
        request_id: question.request_id,
        response: { type: "answer", answers: { [asked]: "Blue" } },
      });
      expect(answered.result).toEqual({});
      const secondTurn = await daemon.waitFor("ask", (e) => e.type === "turn_ended", from);
      expect(secondTurn).toMatchObject({ outcome: "success" });
      const texts = daemon.eventsOf("ask", from).filter((e) => e.type === "assistant_text");
      expect(texts.some((e) => e.type === "assistant_text" && /blue/i.test(e.text))).toBe(true);
      expect(daemon.hookRequests).toEqual([]);
      // Left running on purpose: closing stdin must stop it.
    },
    2 * TURN_TIMEOUT_MS + 60_000,
  );

  test.skipIf(!codexEnabled)(
    "codex: starts, answers pong, and resumes its thread in a new app-server",
    async () => {
      const started = await startCodex("codex-pong", "bypass_permissions");
      expect(started.error).toBeUndefined();
      const threadId = (started.result as { agent_session_id: string | null }).agent_session_id;
      expect(threadId).toBeTruthy();

      await daemon.request("session.send", { chat_id: "codex-pong", text: "Reply with the single word pong" });
      const ended = await daemon.waitFor("codex-pong", (e) => e.type === "turn_ended");
      const events = daemon.eventsOf("codex-pong");
      expect(events[0]).toEqual({ type: "session_started", agent_session_id: threadId as string, model: CODEX_MODEL });
      expect(events.findIndex((e) => e.type === "turn_started")).toBeLessThan(events.indexOf(ended));
      const pong = events.find((e) => e.type === "assistant_text" && /pong/i.test(e.text));
      expect(pong).toBeDefined();
      const deltas = events.filter((e) => e.type === "text_delta" && pong?.type === "assistant_text" && e.message_id === pong.message_id);
      expect(deltas.length).toBeGreaterThan(0);
      expect(ended).toMatchObject({ type: "turn_ended", outcome: "success", error: null, cost_usd: null });
      if (ended.type === "turn_ended") {
        expect(ended.duration_ms).toBeGreaterThan(0);
        expect(ended.context_tokens).toBeGreaterThan(0);
      }
      expect((await daemon.request("session.stop", { chat_id: "codex-pong" })).result).toEqual({});

      // A new app-server picks the thread up again.
      const resumed = await startCodex("codex-pong", "bypass_permissions", { resume_session_id: threadId });
      expect(resumed.result).toMatchObject({ agent_session_id: threadId });
      const from = daemon.eventsOf("codex-pong").length;
      await daemon.request("session.send", { chat_id: "codex-pong", text: "Which single word did you reply before? Reply with that word only." });
      const again = await daemon.waitFor("codex-pong", (e) => e.type === "turn_ended", from);
      expect(again).toMatchObject({ outcome: "success" });
      const recalled = daemon.eventsOf("codex-pong", from).filter((e) => e.type === "assistant_text");
      expect(recalled.some((e) => e.type === "assistant_text" && /pong/i.test(e.text))).toBe(true);
    },
    2 * TURN_TIMEOUT_MS + 60_000,
  );

  test.skipIf(!codexEnabled)(
    "codex: the PreToolUse hook denies git worktree add even when permissions are bypassed",
    async () => {
      const from = daemon.eventsOf("codex-pong").length;
      await daemon.request("session.send", {
        chat_id: "codex-pong",
        text: `Run exactly this shell command once, then stop and reply with what happened: git -C ${repo} worktree add ${repo}-cwt -b codex-e2e-wt`,
      });
      await daemon.waitFor("codex-pong", (e) => e.type === "turn_ended", from);
      const events = daemon.eventsOf("codex-pong", from);
      const blocked = events.find((e) => e.type === "notice" && e.message.startsWith("A command was blocked"));
      expect(blocked?.type === "notice" && blocked.message).toContain("chauffeur worktree add");
      expect(existsSync(`${repo}-cwt`)).toBe(false);
      expect((await daemon.request("session.stop", { chat_id: "codex-pong" })).result).toEqual({});
    },
    TURN_TIMEOUT_MS + 60_000,
  );

  test.skipIf(!codexEnabled)(
    "codex: routes a command approval to the daemon in default mode",
    async () => {
      expect((await startCodex("codex-ask", "default")).error).toBeUndefined();
      await daemon.request("session.send", {
        chat_id: "codex-ask",
        text: 'Run exactly the shell command `touch codex-approved.txt`, requesting escalated permissions (sandbox_permissions require_escalated) with the justification "e2e". Then reply done.',
      });
      const request = await daemon.waitFor("codex-ask", (e) => e.type === "permission_request");
      if (request.type !== "permission_request") throw new Error("unreachable");
      expect(request).toMatchObject({ tool_name: "Bash", title: "e2e", input: { command: expect.stringContaining("touch codex-approved.txt") } });
      const allowed = await daemon.request("session.respond", {
        chat_id: "codex-ask",
        request_id: request.request_id,
        response: { type: "allow", always: false },
      });
      expect(allowed.result).toEqual({});
      const ended = await daemon.waitFor("codex-ask", (e) => e.type === "turn_ended");
      expect(ended).toMatchObject({ outcome: "success" });
      const events = daemon.eventsOf("codex-ask");
      expect(events).toContainEqual({ type: "request_resolved", request_id: request.request_id, outcome: "allowed" });
      const call = events.find((e) => e.type === "tool_call" && e.name === "Bash" && JSON.stringify(e.input).includes("codex-approved"));
      expect(events).toContainEqual(expect.objectContaining({ type: "tool_result", tool_use_id: call?.type === "tool_call" ? call.tool_use_id : "", is_error: false }));
      expect(existsSync(join(repo, "codex-approved.txt"))).toBe(true);
      // Left running on purpose: closing stdin must stop it too.
    },
    TURN_TIMEOUT_MS + 60_000,
  );
  test.skipIf(!codexEnabled)(
    "codex: a trusted repo's own MCP server joins a workspace chat, an untrusted repo's does not",
    async () => {
      // A throwaway HOME and CODEX_HOME: the user's ~/.codex is never read nor written, and no
      // login is needed (no turn runs). Two repos, each a clone with a worktree in the workspace
      // folder; Codex trusts the first clone only.
      const root = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-e2e-repocfg-")));
      try {
        const home = join(root, "home");
        mkdirSync(join(home, ".codex"), { recursive: true });
        const fakeMcp = join(root, "fake-mcp.py");
        writeFileSync(fakeMcp, FAKE_MCP);
        const git = (cwd: string, ...args: string[]) => {
          const run = Bun.spawnSync(["git", "-c", "user.name=e2e", "-c", "user.email=e2e@example.com", ...args], { cwd, env: { ...process.env, HOME: home } });
          if (run.exitCode !== 0) throw new Error(`git ${args.join(" ")} failed: ${run.stderr.toString()}`);
        };
        const worktrees = ["trusted", "untrusted"].map((name) => {
          const clone = join(root, "clones", name);
          mkdirSync(join(clone, ".codex"), { recursive: true });
          // And a server whose header variable is not set: it fails, and says where it is declared.
          writeFileSync(
            join(clone, ".codex", "config.toml"),
            `[mcp_servers.${name}_notes]\ncommand = "python3"\nargs = [${JSON.stringify(fakeMcp)}, "${name}"]\n\n` +
              `[mcp_servers.${name}_remote]\nurl = "http://127.0.0.1:9/mcp"\nenv_http_headers = { Authorization = "CHAUFFEUR_E2E_UNSET_TOKEN" }\n`,
          );
          git(clone, "init", "-q");
          git(clone, "add", ".");
          git(clone, "commit", "-qm", "init");
          const worktree = join(root, "ws", name);
          git(clone, "worktree", "add", "-q", "-b", "ws", worktree);
          return worktree;
        });
        writeFileSync(join(home, ".codex", "config.toml"), `[projects.${JSON.stringify(join(root, "clones", "trusted"))}]\ntrust_level = "trusted"\n`);

        const started = await startCodex("codex-repocfg", "default", {
          cwd: join(root, "ws"),
          additional_directories: worktrees,
          env: { CHAUFFEUR_WORKSPACE_ID: "w-e2e", HOME: home, CODEX_HOME: join(home, ".codex") },
        });
        expect(started.error).toBeUndefined();
        const status = await daemon.waitFor(
          "codex-repocfg",
          (e) =>
            e.type === "mcp_status" &&
            e.servers.some((server) => server.name === "trusted_notes" && server.status === "connected") &&
            e.servers.some((server) => server.name === "trusted_remote" && server.status === "failed"),
          0,
          45_000,
        );
        if (status.type !== "mcp_status") throw new Error("not a status");
        expect(status.servers.map((server) => server.name).sort()).toEqual(["trusted_notes", "trusted_remote"]);
        expect(status.servers.find((server) => server.name === "trusted_remote")).toMatchObject({
          missing_env: ["CHAUFFEUR_E2E_UNSET_TOKEN"],
          declared_in: `${worktrees[0]}/.codex/config.toml [mcp_servers.trusted_remote]`,
        });
        const notice = daemon.eventsOf("codex-repocfg").find((e) => e.type === "notice" && e.message.startsWith("untrusted's .codex/config.toml is not loaded"));
        expect(notice).toBeDefined();
        expect((await daemon.request("session.stop", { chat_id: "codex-repocfg" })).result).toEqual({});
      } finally {
        rmSync(root, { recursive: true, force: true });
      }
    },
    60_000,
  );
});
