import { describe as group, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { AcpEventMapper, type AcpToolCall, describe } from "../src/agents/acp/events.ts";
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
  const context: PermissionContext = { mode: "auto", readOnly: false, designDir: undefined, cwd: "/w", roots: ["/w", "/w/web"], clones: [] };
  const shell = (command: string) => describe({ toolCallId: "c", kind: "execute", rawInput: { command } });
  const edit = (path: string) => describe({ toolCallId: "e", kind: "edit", content: [{ type: "diff", path, oldText: "a", newText: "b" }] });

  test("an edit or a shell command writing into a source clone is refused whatever the mode, naming the worktree", () => {
    const guarded: PermissionContext = { ...context, clones: [{ clone: "/code/web", worktree: "/w/web" }] };
    for (const mode of ["auto", "bypass_permissions", "default", "accept_edits"] as const) {
      const refused = decide(edit("/code/web/src/app.ts"), { ...guarded, mode });
      expect(refused).toEqual({ type: "deny", reason: expect.stringContaining("Write /w/web/src/app.ts instead") });
      expect(decide(shell("echo x > /code/web/notes.md"), { ...guarded, mode }).type).toBe("deny");
      expect(decide(shell("cd /code/web && git commit -am x"), { ...guarded, mode }).type).toBe("deny");
    }
    // Reads of the clone, and writes in the worktree, as before.
    expect(decide(shell("cat /code/web/README.md"), { ...guarded, mode: "bypass_permissions" }).type).toBe("allow");
    expect(decide(edit("/w/web/src/app.ts"), guarded).type).toBe("allow");
  });

  test("a process's own folder and variables, and what is written to one, count for the clone guard", () => {
    const guarded: PermissionContext = { ...context, mode: "bypass_permissions", clones: [{ clone: "/code/web", worktree: "/w/web" }] };
    const process = (rawInput: Record<string, unknown>) => describe({ toolCallId: "p", kind: "execute", title: "Running", rawInput: rawInput as never });
    expect(decide(process({ command: "touch notes.md", cwd: "/code/web" }), guarded).type).toBe("deny");
    expect(decide(process({ command: "git commit -am wip", cwd: "/code/web" }), guarded).type).toBe("deny");
    expect(decide(process({ command: "git clean -fdx", env: { GIT_WORK_TREE: "/code/web", GIT_DIR: "/code/web/.git" } }), guarded).type).toBe("deny");
    expect(decide(process({ processId: "ab", text: "echo x > /code/web/a\n" }), guarded).type).toBe("deny");
    expect(decide(process({ processId: "ab", bytesBase64: Buffer.from("echo x > /code/web/a\n").toString("base64") }), guarded).type).toBe("deny");
    // A process's variables hold for every command of its line, as an exported variable does.
    expect(decide(process({ command: "true; git clean -fdx", env: { GIT_WORK_TREE: "/code/web", GIT_DIR: "/code/web/.git" } }), guarded).type).toBe("deny");
    // A variable whose name no shell could hold does not hide the command from the guard.
    for (const name of ["#", "A;#", "A B", "1A", "A=B"]) {
      expect(decide(process({ command: "touch /code/web/a", env: { [name]: "x" } }), guarded).type).toBe("deny");
    }
    // In the worktree, or reading the clone: as before.
    expect(decide(process({ command: "touch notes.md", cwd: "/w/web" }), guarded).type).toBe("allow");
    expect(decide(process({ command: "ls", cwd: "/code/web" }), guarded).type).toBe("allow");
  });

  test("a permission is decided on the call as its updates left it", () => {
    const guarded: PermissionContext = { ...context, mode: "bypass_permissions", clones: [{ clone: "/code/web", worktree: "/w/web" }] };
    const mapper = new AcpEventMapper();
    mapper.map({ sessionUpdate: "tool_call", toolCallId: "u1", title: "Running", kind: "execute", status: "pending" });
    mapper.map({ sessionUpdate: "tool_call_update", toolCallId: "u1", rawInput: { command: "touch /code/web/a" } });
    expect(decide(mapper.tool("u1") as AcpToolCall, guarded).type).toBe("deny");
    mapper.map({ sessionUpdate: "tool_call", toolCallId: "u2", title: "Writing", kind: "edit", status: "pending" });
    mapper.map({ sessionUpdate: "tool_call_update", toolCallId: "u2", locations: [{ path: "/code/web/a.ts" }] });
    expect(decide(mapper.tool("u2") as AcpToolCall, guarded).type).toBe("deny");
    // What the update leaves out stays.
    expect(mapper.tool("u2")?.title).toBe("Writing");
  });

  test("input written to a running process runs in that process's folder, with its variables", () => {
    const guarded: PermissionContext = { ...context, mode: "bypass_permissions", clones: [{ clone: "/code/web", worktree: "/w/web" }] };
    const mapper = new AcpEventMapper();
    const start = (id: string, rawInput: Record<string, unknown>, processId: string) => {
      mapper.map({ sessionUpdate: "tool_call", toolCallId: id, title: "Starting", kind: "execute", status: "pending", rawInput: rawInput as never });
      mapper.map({ sessionUpdate: "tool_call_update", toolCallId: id, status: "completed", rawOutput: { processId, status: "running" } });
    };
    const write = (id: string, processId: string, text: string) => {
      mapper.map({ sessionUpdate: "tool_call", toolCallId: id, title: "Writing", kind: "execute", status: "pending", rawInput: { processId, text } });
      return decide(mapper.tool(id) as AcpToolCall, guarded).type;
    };
    start("s1", { command: "bash", cwd: "/code/web" }, "in-clone");
    start("s2", { command: "bash", env: { GIT_WORK_TREE: "/code/web", GIT_DIR: "/code/web/.git" } }, "with-git-vars");
    start("s3", { command: "bash", cwd: "/w/web" }, "in-worktree");
    expect(write("w1", "in-clone", "touch notes.md\n")).toBe("deny");
    expect(write("w2", "with-git-vars", "git clean -fdx\n")).toBe("deny");
    expect(write("w3", "in-worktree", "touch notes.md\n")).toBe("allow");
    expect(write("w4", "in-clone", "ls\n")).toBe("allow");
  });

  test("an edit whose file is only in its input is checked too", () => {
    const guarded: PermissionContext = { ...context, mode: "bypass_permissions", clones: [{ clone: "/code/web", worktree: "/w/web" }] };
    for (const name of ["path", "file_path", "filePath"]) {
      const tool = describe({ toolCallId: "r", kind: "edit", title: "Editing", rawInput: { [name]: "/code/web/a.ts" } });
      expect(decide(tool, guarded).type).toBe("deny");
    }
  });

  test("a call the agent said nothing of is put to the user, whatever the mode", () => {
    const unknown = describe({ toolCallId: "write_file" });
    for (const mode of ["auto", "bypass_permissions", "accept_edits"] as const) expect(decide(unknown, { ...context, mode }).type).toBe("ask");
    expect(decide(unknown, { ...context, readOnly: true }).type).toBe("ask");
    expect(decide(unknown, { ...context, designDir: "/w/.chauffeur/design" }).type).toBe("ask");
  });

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
