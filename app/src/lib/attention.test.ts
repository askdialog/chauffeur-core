import { describe, expect, test } from "bun:test";
import type { WorkspaceInfo } from "@chauffeur/protocol";
import { attentionText, chatAttention, needingYou } from "./attention";

describe("chatAttention", () => {
  test("a finished, failed or blocked turn needs attention", () => {
    expect(chatAttention("working", "idle")).toBe("finished");
    expect(chatAttention("starting", "error")).toBe("failed");
    expect(chatAttention("working", "needs_input")).toBe("needs_input");
    expect(chatAttention("needs_input", "idle")).toBe("finished");
  });

  test("other changes do not", () => {
    expect(chatAttention("idle", "working")).toBeNull();
    expect(chatAttention("idle", "idle")).toBeNull();
    expect(chatAttention("idle", "closed")).toBeNull();
    expect(chatAttention("working", "closed")).toBeNull();
    expect(chatAttention(undefined, "idle")).toBeNull();
  });

  test("an unknown chat falls back on the workspace's activity", () => {
    expect(chatAttention(undefined, "idle", "working")).toBe("finished");
    expect(chatAttention(undefined, "idle", "idle")).toBeNull();
    expect(chatAttention(undefined, "needs_input", "working")).toBe("needs_input");
  });

  test("texts", () => {
    expect(attentionText("needs_input", "Claude")).toBe("Claude needs your input");
    expect(attentionText("finished", "Codex")).toBe("Codex finished");
  });
});

describe("needingYou", () => {
  const ws = (id: string, more: Partial<WorkspaceInfo>) => ({ id, state: "ready", activity: "idle", ...more }) as WorkspaceInfo;

  test("counts the workspaces the sidebar badges as needing input", () => {
    const workspaces = {
      a: ws("a", { activity: "needs_input" }),
      b: ws("b", { activity: "needs_input", unread: true }),
      c: ws("c", { activity: "working" }),
      d: ws("d", { activity: "idle", unread: true }),
      e: ws("e", { activity: "error" }),
    };
    expect(needingYou(workspaces)).toBe(2);
  });

  test("a workspace being archived or archived does not count", () => {
    expect(needingYou({ a: ws("a", { activity: "needs_input", state: "archiving" }), b: ws("b", { activity: "needs_input", state: "archived" }) })).toBe(0);
  });
});
