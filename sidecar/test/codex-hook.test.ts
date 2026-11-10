import { describe, expect, test } from "bun:test";
import { resolveCodexExecutable } from "../src/agents/codex/executable.ts";
import { hookCommandLine, preToolUseAnswer } from "../src/agents/codex/hook.ts";
import { appServerArgs, daemonSocket, SANDBOX_NOTE, sandboxNetworkOpen, sandboxPolicy, threadConfig } from "../src/agents/codex/options.ts";
import { ErrorCode } from "../src/protocol.ts";

const payload = (command: unknown) =>
  JSON.stringify({ hook_event_name: "PreToolUse", tool_name: "Bash", tool_input: { command }, cwd: "/w", permission_mode: "bypassPermissions" });

describe("the Codex PreToolUse hook", () => {
  test("denies git worktree add with the Chauffeur command to use", () => {
    const answer = preToolUseAnswer(payload("cd alpha && git -C . worktree add ../x -b y"));
    expect(JSON.parse(answer as string)).toEqual({
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "deny",
        permissionDecisionReason: expect.stringContaining("chauffeur worktree add --repo <repo> [--name <name>]"),
      },
    });
  });

  test("denies chauffeur add-repo, naming the add_repo tool", () => {
    const answer = JSON.parse(preToolUseAnswer(payload("chauffeur add-repo w analytics")) as string);
    expect(answer.hookSpecificOutput.permissionDecision).toBe("deny");
    expect(answer.hookSpecificOutput.permissionDecisionReason).toContain("add_repo tool");
  });

  test("lets every other command run", () => {
    expect(preToolUseAnswer(payload("git worktree list"))).toBeUndefined();
    expect(preToolUseAnswer(payload("chauffeur worktree add --repo alpha"))).toBeUndefined();
    expect(preToolUseAnswer(payload(undefined))).toBeUndefined();
    expect(preToolUseAnswer("not json")).toBeUndefined();
  });

  test("in an exploration, also denies git and GitHub writes", () => {
    expect(preToolUseAnswer(payload("git push origin HEAD"))).toBeUndefined();
    expect(preToolUseAnswer(payload("git push origin HEAD"), "read-only")).toContain("exploration, read-only");
    expect(preToolUseAnswer(payload(["bash", "-lc", "gh pr create --fill"]), "read-only")).toBeDefined();
    expect(preToolUseAnswer(payload("git log -5"), "read-only")).toBeUndefined();
    expect(hookCommandLine("read-only")).toEndWith(" codex-hook pre-tool-use read-only");
  });

  test("in a design chat, denies git and GitHub writes with the design chat's reason", () => {
    const answer = JSON.parse(preToolUseAnswer(payload("git commit -am wip"), "design") as string);
    expect(answer.hookSpecificOutput.permissionDecision).toBe("deny");
    expect(answer.hookSpecificOutput.permissionDecisionReason).toContain("design chat");
    expect(preToolUseAnswer(payload(["bash", "-lc", "gh pr create --fill"]), "design")).toBeDefined();
    expect(preToolUseAnswer(payload("git log -5"), "design")).toBeUndefined();
    expect(hookCommandLine("design")).toEndWith(" codex-hook pre-tool-use design");
  });

  test("reads argv-style commands too", () => {
    expect(preToolUseAnswer(payload(["bash", "-lc", "git worktree add ../x"]))).toBeDefined();
    expect(preToolUseAnswer(payload(["git", "worktree", "add", "../x"]))).toBeDefined();
    expect(preToolUseAnswer(payload(["git", "worktree", "prune"]))).toBeUndefined();
  });

  test("is registered on Bash with a quoted command line", () => {
    const line = hookCommandLine();
    expect(line).toEndWith(" codex-hook pre-tool-use");
    expect(line.startsWith(process.execPath) || line.startsWith(`'${process.execPath}'`)).toBe(true);
    const [flag, value] = appServerArgs(`'/Apps/My "Sidecar"/bin' codex-hook pre-tool-use`);
    expect(flag).toBe("-c");
    expect(value).toBe(`hooks.PreToolUse=[{matcher="Bash", hooks=[{type="command", command="'/Apps/My \\"Sidecar\\"/bin' codex-hook pre-tool-use"}]}]`);
    expect(appServerArgs(undefined)).toEqual([]);
  });
});

describe("Codex settings", () => {
  test("sandbox policies of the permission modes", () => {
    expect(sandboxPolicy("bypass_permissions", ["/w"])).toEqual({ type: "dangerFullAccess" });
    expect(sandboxPolicy("plan", ["/w"])).toEqual({ type: "readOnly", networkAccess: false });
    expect(sandboxPolicy("default", ["/w", "/x"])).toEqual({
      type: "workspaceWrite",
      writableRoots: ["/w", "/x"],
      networkAccess: false,
      excludeTmpdirEnvVar: false,
      excludeSlashTmp: false,
    });
  });

  test("the daemon's socket is for a chat given the CLI, never an exploration's", () => {
    expect(daemonSocket({ CHAUFFEUR_HOME: "/home/me/.chauffeur" }, false, "darwin")).toBe("/home/me/.chauffeur/chauffeurd.sock");
    // An exploration's agent gets no CHAUFFEUR_HOME; read-only, it gets no socket whatever its environment.
    expect(daemonSocket({ CHAUFFEUR_WORKSPACE_ID: "ws-1" }, false, "darwin")).toBeUndefined();
    expect(daemonSocket({ CHAUFFEUR_HOME: "/home/me/.chauffeur" }, true, "darwin")).toBeUndefined();
    expect(daemonSocket(undefined, false, "darwin")).toBeUndefined();
    // Codex's Linux sandbox allows every Unix socket or none.
    expect(daemonSocket({ CHAUFFEUR_HOME: "/home/me/.chauffeur" }, false, "linux")).toBeUndefined();
  });

  test("allows only the daemon's socket, through Codex's network proxy", () => {
    expect(threadConfig(["/w/alpha"], undefined, false, "/h/chauffeurd.sock")).toEqual({
      "sandbox_workspace_write.writable_roots": ["/w/alpha"],
      "features.network_proxy": { enabled: true, unix_sockets: { "/h/chauffeurd.sock": "allow" } },
      "sandbox_workspace_write.network_access": true,
    });
    expect(threadConfig([], undefined, false)).toEqual({});
    expect(SANDBOX_NOTE).toContain("a `chauffeur` command that says a sandbox blocked");
  });

  test("tells a config that already opens the sandbox's network", () => {
    expect(sandboxNetworkOpen({ sandbox_workspace_write: { network_access: true } })).toBe(true);
    expect(sandboxNetworkOpen({ sandbox_workspace_write: { network_access: false } })).toBe(false);
    expect(sandboxNetworkOpen({ sandbox_workspace_write: {} })).toBe(false);
    expect(sandboxNetworkOpen({})).toBe(false);
    expect(sandboxNetworkOpen(null)).toBe(false);
  });

  test("resolves codex from CHAUFFEUR_CODEX_PATH, else from the agent's PATH", () => {
    const lookup = (found: string | null, existing: string[] = []) => ({
      which: (_: string, options: { PATH: string }) => (options.PATH.includes("/opt/bin") ? found : null),
      exists: (path: string) => existing.includes(path),
    });
    expect(resolveCodexExecutable({ CHAUFFEUR_CODEX_PATH: "/pinned/codex", PATH: "/opt/bin" }, lookup("/opt/bin/codex", ["/pinned/codex"]))).toBe("/pinned/codex");
    expect(() => resolveCodexExecutable({ CHAUFFEUR_CODEX_PATH: "/missing" }, lookup(null))).toThrow(expect.objectContaining({ code: ErrorCode.UNAVAILABLE }));
    expect(resolveCodexExecutable({ PATH: "/usr/bin:/opt/bin" }, lookup("/opt/bin/codex"))).toBe("/opt/bin/codex");
    expect(() => resolveCodexExecutable({ PATH: "/usr/bin" }, lookup("/opt/bin/codex"))).toThrow("codex is not installed");
  });
});
