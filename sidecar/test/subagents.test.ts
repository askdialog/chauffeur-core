import { describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { SDKMessage, SessionMessage } from "@anthropic-ai/claude-agent-sdk";
import { buildClaudeOptions } from "../src/agents/claude/options.ts";
import { ClaudeEventMapper } from "../src/agents/claude/events.ts";
import { agentIdIn, claudeSubagentMessages, type TranscriptReader, transcriptEvents } from "../src/agents/claude/subagents.ts";
import type { ChatEvent } from "../src/protocol.ts";
import { sdk } from "./support/fakes.ts";

const SESSION = "db8d5562-05cf-4bce-9d6c-35b6eefb5d1a";
const AGENT = "aa76c5dc444b7828c";
const FIXTURE = join(import.meta.dir, "fixtures", "claude-subagent.jsonl");

/** The fixture as the SDK returns it: user and assistant rows, in order. */
function sessionMessages(): SessionMessage[] {
  return readFileSync(FIXTURE, "utf8")
    .trim()
    .split("\n")
    .map((line) => JSON.parse(line))
    .filter((row) => row.type === "user" || row.type === "assistant")
    .map((row) => ({
      type: row.type,
      uuid: row.uuid,
      session_id: row.sessionId,
      message: row.message,
      parent_tool_use_id: "toolu_agent",
      parent_agent_id: null,
    }));
}

/** The same conversation as Claude Code forwards it live with `forwardSubagentText`. */
function forwarded(): SDKMessage[] {
  return sessionMessages().map(
    (m) => ({ type: m.type, message: m.message, parent_tool_use_id: "toolu_agent", uuid: m.uuid, session_id: SESSION }) as unknown as SDKMessage,
  );
}

function reader(transcripts: Record<string, SessionMessage[]>): TranscriptReader & { reads: string[] } {
  const reads: string[] = [];
  return {
    reads,
    async messages(sessionId, agentId, dir, limit) {
      reads.push(`${sessionId}/${agentId}@${dir}${limit ? `:${limit}` : ""}`);
      const all = transcripts[agentId] ?? [];
      return limit ? all.slice(0, limit) : all;
    },
    async list() {
      return Object.keys(transcripts);
    },
  };
}

describe("a subagent's text, live", () => {
  test("sessions ask Claude Code to forward it", () => {
    const options = buildClaudeOptions(
      {
        chat_id: "c",
        workspace_id: "w",
        agent: "claude",
        cwd: "/w",
        additional_directories: [],
        fast: false,
        permission_mode: "auto",
        read_only: false,
        env: {},
        prompt_attachments: [],
        prompt_suggestions: false,
      },
      {
        baseEnv: {},
        executable: undefined,
        canUseTool: async () => ({ behavior: "deny", message: "" }),
        hooks: {},
        abortController: new AbortController(),
        stderr: () => {},
        jevPluginDir: () => "/jev",
      },
    );
    expect(options.forwardSubagentText).toBe(true);
  });

  test("is tied to the tool call that started the subagent, the agent's own is not", () => {
    const mapper = new ClaudeEventMapper();
    const events = [sdk.text("msg_main", "Starting a subagent."), ...forwarded()].flatMap((m) => mapper.map(m));
    expect(events.filter((e) => e.type === "assistant_text" || e.type === "thinking")).toEqual([
      { type: "assistant_text", message_id: "msg_main:0", text: "Starting a subagent.", parent_tool_use_id: null },
      { type: "thinking", message_id: "msg_A:0", text: "A lighthouse sentence first.", parent_tool_use_id: "toolu_agent" },
      { type: "assistant_text", message_id: "msg_A:1", text: "Lighthouses have guided sailors for centuries.", parent_tool_use_id: "toolu_agent" },
      { type: "assistant_text", message_id: "msg_B:0", text: "Done: `echo hi` printed `hi`.", parent_tool_use_id: "toolu_agent" },
    ]);
    // Tool calls stay as they were: in the chat, under the subagent's call.
    expect(events.find((e) => e.type === "tool_call")).toMatchObject({ tool_use_id: "toolu_bash", parent_tool_use_id: "toolu_agent" });
  });

  test("the agent's own thinking carries no parent, as before", () => {
    const thinking = {
      type: "assistant",
      parent_tool_use_id: null,
      message: { id: "msg_t", content: [{ type: "thinking", thinking: "hmm" }] },
    } as unknown as SDKMessage;
    expect(new ClaudeEventMapper().map(thinking)).toEqual([{ type: "thinking", message_id: "msg_t:0", text: "hmm" }]);
  });

  test("a subagent's stream, if Claude Code streams one, goes to its view too", () => {
    const mapper = new ClaudeEventMapper();
    const stream = (event: unknown) => ({ type: "stream_event", parent_tool_use_id: "toolu_agent", event }) as unknown as SDKMessage;
    const events = [
      stream({ type: "message_start", message: { id: "msg_S", usage: {} } }),
      stream({ type: "content_block_start", index: 0, content_block: { type: "text", text: "" } }),
      stream({ type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Light" } }),
    ].flatMap((m) => mapper.map(m));
    expect(events).toEqual([{ type: "text_delta", message_id: "msg_S:0", text: "Light", parent_tool_use_id: "toolu_agent" }]);
  });
});

describe("a subagent's transcript", () => {
  test("maps to the events the live mapping gives, as the subagent's own", () => {
    const events = transcriptEvents(sessionMessages());
    expect(events).toEqual([
      { type: "thinking", message_id: "msg_A:0", text: "A lighthouse sentence first." },
      { type: "assistant_text", message_id: "msg_A:1", text: "Lighthouses have guided sailors for centuries.", parent_tool_use_id: null },
      { type: "tool_call", tool_use_id: "toolu_bash", name: "Bash", input: { command: "echo hi", description: "Print hi" }, parent_tool_use_id: null },
      { type: "tool_result", tool_use_id: "toolu_bash", output: "hi", is_error: false },
      { type: "assistant_text", message_id: "msg_B:0", text: "Done: `echo hi` printed `hi`.", parent_tool_use_id: null },
    ]);
    // Same ids as live: the app merges both without duplicates.
    const mapper = new ClaudeEventMapper();
    const live = forwarded().flatMap((m) => mapper.map(m));
    const ids = (list: ChatEvent[]) => list.flatMap((e) => (e.type === "assistant_text" || e.type === "thinking" ? [e.message_id] : []));
    expect(ids(events)).toEqual(ids(live));
    expect(ids(events)).toEqual(["msg_A:0", "msg_A:1", "msg_B:0"]);
  });

  test("leaves out a tool result's images and malformed rows", () => {
    const messages: SessionMessage[] = [
      { type: "assistant", uuid: "a", session_id: SESSION, message: null, parent_tool_use_id: null, parent_agent_id: null },
      {
        type: "user",
        uuid: "b",
        session_id: SESSION,
        parent_tool_use_id: null,
        parent_agent_id: null,
        message: {
          role: "user",
          content: [{ type: "tool_result", tool_use_id: "t", content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: "AAAA" } }] }],
        },
      },
    ];
    expect(transcriptEvents(messages)).toEqual([{ type: "tool_result", tool_use_id: "t", output: "[image]", is_error: false }]);
  });

  test("is found by the agent id given", async () => {
    const transcripts = reader({ [AGENT]: sessionMessages() });
    const result = await claudeSubagentMessages(transcripts, { agent: "claude", session_id: SESSION, cwd: "/w", agent_id: AGENT });
    expect(result.agent_id).toBe(AGENT);
    expect(result.found).toBe(true);
    expect(result.events).toHaveLength(5);
    expect(transcripts.reads).toEqual([`${SESSION}/${AGENT}@/w`]);
  });

  test("is found by the agent id its tool result names", async () => {
    const result = await claudeSubagentMessages(reader({ [AGENT]: sessionMessages() }), {
      agent: "claude",
      session_id: SESSION,
      cwd: "/w",
      result: `Done.\nagentId: ${AGENT} (use SendMessage with to: '${AGENT}' to continue this agent)\n<usage>tool_uses: 1</usage>`,
    });
    expect(result).toMatchObject({ agent_id: AGENT, found: true });
    expect(agentIdIn("Async agent launched successfully.\nagentId: a7b9a0d00fbdf5d04 (internal ID)")).toBe("a7b9a0d00fbdf5d04");
    expect(agentIdIn('This agent\'s report was delivered to you as a message from "a84b4b71b9b5b3e0a" (its SubagentHandback call).')).toBe("a84b4b71b9b5b3e0a");
    expect(agentIdIn("no id here")).toBeNull();
  });

  test("is found by its prompt when nothing names it (older chats)", async () => {
    const other: SessionMessage[] = [{ ...sessionMessages()[0]!, message: { role: "user", content: "Something else" } }];
    const result = await claudeSubagentMessages(reader({ aother: other, [AGENT]: sessionMessages() }), {
      agent: "claude",
      session_id: SESSION,
      cwd: "/w",
      prompt: "Write one sentence about lighthouses, run 'echo hi', then report.\n",
    });
    expect(result).toMatchObject({ agent_id: AGENT, found: true });
  });

  test("missing: a clear empty answer", async () => {
    const empty = reader({});
    expect(await claudeSubagentMessages(empty, { agent: "claude", session_id: SESSION, cwd: "/w", agent_id: "agone" })).toEqual({
      agent_id: "agone",
      found: false,
      events: [],
    });
    expect(await claudeSubagentMessages(empty, { agent: "claude", session_id: SESSION, cwd: "/w", prompt: "who?" })).toEqual({
      agent_id: null,
      found: false,
      events: [],
    });
    expect(await claudeSubagentMessages(empty, { agent: "claude", session_id: SESSION, cwd: "/w" })).toEqual({
      agent_id: null,
      found: false,
      events: [],
    });
  });

  test("refuses ids that would name another file", async () => {
    const params = { agent: "claude" as const, session_id: SESSION, cwd: "/w" };
    await expect(claudeSubagentMessages(reader({}), { ...params, agent_id: "../../etc/passwd" })).rejects.toMatchObject({ code: -32602 });
    await expect(claudeSubagentMessages(reader({}), { ...params, session_id: "../x" })).rejects.toMatchObject({ code: -32602 });
  });

  test("is read from Claude Code's folder with the SDK's own readers", async () => {
    const home = mkdtempSync(join(tmpdir(), "chauffeur-claude-home-"));
    const cwd = realpathSync(mkdtempSync(join(tmpdir(), "chauffeur-workspace-")));
    try {
      const folder = join(home, "projects", cwd.replace(/[^a-zA-Z0-9]/g, "-"), SESSION, "subagents");
      mkdirSync(folder, { recursive: true });
      // Claude Code finds a session's subagents beside its own transcript.
      writeFileSync(
        join(folder, "..", "..", `${SESSION}.jsonl`),
        `${JSON.stringify({ type: "user", uuid: "m1", parentUuid: null, sessionId: SESSION, cwd, message: { role: "user", content: "hi" } })}\n`,
      );
      writeFileSync(join(folder, `agent-${AGENT}.jsonl`), readFileSync(FIXTURE, "utf8").replaceAll('"cwd": "/w"', `"cwd": ${JSON.stringify(cwd)}`));
      const read = async (params: Record<string, unknown>) => {
        const child = Bun.spawn(["bun", join(import.meta.dir, "support", "read-subagent.ts"), JSON.stringify({ agent: "claude", session_id: SESSION, cwd, ...params })], {
          env: { ...process.env, CLAUDE_CONFIG_DIR: home },
          stdout: "pipe",
          stderr: "pipe",
        });
        const out = await new Response(child.stdout).text();
        expect(await child.exited).toBe(0);
        return JSON.parse(out) as { agent_id: string | null; found: boolean; events: ChatEvent[] };
      };
      const byId = await read({ agent_id: AGENT });
      expect(byId.found).toBe(true);
      expect(byId.events.map((e) => e.type)).toEqual(["thinking", "assistant_text", "tool_call", "tool_result", "assistant_text"]);
      expect(await read({ prompt: "Write one sentence about lighthouses, run 'echo hi', then report." })).toMatchObject({ agent_id: AGENT, found: true });
      expect(await read({ agent_id: "anotthere" })).toEqual({ agent_id: "anotthere", found: false, events: [] });
    } finally {
      rmSync(home, { recursive: true, force: true });
      rmSync(cwd, { recursive: true, force: true });
    }
  });
});
