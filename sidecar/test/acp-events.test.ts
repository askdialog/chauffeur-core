import { describe as group, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AcpEventMapper, describe } from "../src/agents/acp/events.ts";
import { decide, type PermissionContext } from "../src/agents/acp/permissions.ts";

group("AcpEventMapper", () => {
  test("text and thoughts stream under one id each, and are given whole when something else comes", () => {
    const mapper = new AcpEventMapper();
    const events = [
      ...mapper.map({ sessionUpdate: "agent_thought_chunk", content: { type: "text", text: "Hmm" } }),
      ...mapper.map({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Bon" } }),
      ...mapper.map({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: "jour" } }),
      ...mapper.map({ sessionUpdate: "tool_call", toolCallId: "t1", title: "Reading a.ts", kind: "read", status: "pending", locations: [{ path: "/w/a.ts" }] }),
    ];
    expect(events.map((event) => event.type)).toEqual(["thinking_delta", "thinking", "text_delta", "text_delta", "assistant_text", "tool_call"]);
    expect(events[4]).toMatchObject({ text: "Bonjour" });
    expect(events[5]).toMatchObject({ name: "Read", input: { file_path: "/w/a.ts" } });
  });

  test("calls are named as Claude's: shell, edits, new files, MCP tools, tool search", () => {
    expect(describe({ toolCallId: "1", title: "bash: ls", kind: "execute", rawInput: { command: "ls -la" } })).toMatchObject({ name: "Bash", input: { command: "ls -la" }, command: "ls -la" });
    expect(describe({ toolCallId: "2", kind: "edit", content: [{ type: "diff", path: "/w/a.ts", oldText: "a", newText: "b" }] })).toMatchObject({
      name: "Edit",
      input: { file_path: "/w/a.ts", old_string: "a", new_string: "b" },
      paths: ["/w/a.ts"],
    });
    expect(describe({ toolCallId: "3", kind: "edit", content: [{ type: "diff", path: "/w/new.ts", newText: "x" }] })).toMatchObject({ name: "Write", input: { file_path: "/w/new.ts", content: "x" } });
    expect(describe({ toolCallId: "4", title: "mcp_chauffeur.delegate", kind: "other", rawInput: { prompt: "Review" } })).toMatchObject({ name: "mcp__chauffeur__delegate", input: { prompt: "Review" } });
    expect(describe({ toolCallId: "5", title: "Searching for relevant tools", kind: "other" })).toMatchObject({ name: "ToolSearch" });
  });

  test("a call's result is its text, or the shell's output, and a failure is an error", () => {
    const mapper = new AcpEventMapper();
    mapper.map({ sessionUpdate: "tool_call", toolCallId: "t1", title: "bash: ls", kind: "execute", status: "in_progress", rawInput: { command: "ls" } });
    expect(mapper.map({ sessionUpdate: "tool_call_update", toolCallId: "t1", status: "completed", rawOutput: { stdout: "a.ts\n", stderr: "" } })).toEqual([
      { type: "tool_result", tool_use_id: "t1", output: "a.ts\n", is_error: false },
    ]);
    mapper.map({ sessionUpdate: "tool_call", toolCallId: "t2", title: "mcp_chauffeur.list_repos", kind: "other", status: "in_progress" });
    expect(
      mapper.map({
        sessionUpdate: "tool_call_update",
        toolCallId: "t2",
        status: "failed",
        content: [
          { type: "content", content: { type: "text", text: "boom" } },
          { type: "content", content: { type: "text", text: "mcp_chauffeur.list_repos" } },
        ],
      }),
    ).toEqual([{ type: "tool_result", tool_use_id: "t2", output: "boom", is_error: true }]);
  });

  test("context readings are given once each, and the turn's end closes what is open", () => {
    const mapper = new AcpEventMapper();
    expect(mapper.map({ sessionUpdate: "usage_update", used: 2000, size: 200000 })).toHaveLength(1);
    expect(mapper.map({ sessionUpdate: "usage_update", used: 2000, size: 200000 })).toHaveLength(0);
    mapper.map({ sessionUpdate: "tool_call", toolCallId: "t1", title: "bash: sleep 99", kind: "execute", status: "in_progress" });
    const ended = mapper.endTurn({ response: { stopReason: "max_tokens", usage: { totalTokens: 9 } } }, 10);
    expect(ended.map((event) => event.type)).toEqual(["tool_result", "turn_ended"]);
    expect(ended[1]).toMatchObject({ outcome: "error", tokens: 9, context_tokens: 2000, error: "The model reached its output limit." });
  });

  test("a plan becomes the chat's plan", () => {
    const mapper = new AcpEventMapper();
    expect(mapper.map({ sessionUpdate: "plan", entries: [{ content: "Read", status: "completed" }, { content: "Fix", status: "in_progress" }] })).toEqual([
      { type: "plan_updated", steps: [{ text: "Read", status: "completed" }, { text: "Fix", status: "in_progress" }] },
    ]);
  });
});

group("decide", () => {
  const context: PermissionContext = { mode: "auto", readOnly: false, designDir: undefined, cwd: "/w", roots: ["/w", "/w/web"] };
  const shell = (command: string) => describe({ toolCallId: "c", kind: "execute", rawInput: { command } });
  const edit = (path: string) => describe({ toolCallId: "e", kind: "edit", content: [{ type: "diff", path, oldText: "a", newText: "b" }] });

  test("worktrees and add-repo are refused whatever the mode", () => {
    for (const mode of ["auto", "bypass_permissions", "default"] as const) {
      expect(decide(shell("git worktree add ../x"), { ...context, mode }).type).toBe("deny");
      expect(decide(shell("chauffeur add-repo web"), { ...context, mode }).type).toBe("deny");
    }
  });

  test("edits run inside the workspace's folders in auto, ask elsewhere and in default; shell asks but in bypass", () => {
    expect(decide(edit("/w/web/a.ts"), context)).toEqual({ type: "allow" });
    expect(decide(edit("/etc/hosts"), context)).toEqual({ type: "ask" });
    expect(decide(edit("/w/web/../../etc/hosts"), context)).toEqual({ type: "ask" });
    expect(decide(edit("/w/web/a.ts"), { ...context, mode: "default" })).toEqual({ type: "ask" });
    expect(decide(shell("bun test"), context)).toEqual({ type: "ask" });
    expect(decide(shell("bun test"), { ...context, mode: "bypass_permissions" })).toEqual({ type: "allow" });
  });

  test("an exploration edits nothing and refuses git writes; a design chat writes its folder only", () => {
    const exploration = { ...context, readOnly: true };
    expect(decide(edit("/w/web/a.ts"), exploration).type).toBe("deny");
    expect(decide(shell("git push"), exploration).type).toBe("deny");
    expect(decide(shell("git log"), exploration)).toEqual({ type: "ask" });
    const workspace = realpathSync(mkdtempSync(join(tmpdir(), "acp-design-")));
    const folder = join(workspace, ".chauffeur", "designs", "c1");
    mkdirSync(folder, { recursive: true });
    const design = { ...context, designDir: folder, cwd: folder };
    expect(decide(edit(join(folder, "home.html")), design)).toEqual({ type: "allow" });
    expect(decide(edit("/w/web/a.ts"), design).type).toBe("deny");
  });
});
