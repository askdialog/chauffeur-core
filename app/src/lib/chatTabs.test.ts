import { describe, expect, test } from "bun:test";
import type { ChatInfo } from "@chauffeur/protocol";
import {
  agentBusy,
  chatLabel,
  chatTabStatus,
  chatsIndicator,
  chatToReopen,
  closedChats,
  neighbourTab,
  openChats,
  forkRefusal,
  placedChats,
  shownChat,
  tabActions,
  withoutChat,
} from "./chatTabs";

const chat = (id: string, more: Partial<ChatInfo> = {}): ChatInfo => ({
  id,
  kind: "chat",
  workspace_id: "w",
  agent: "claude",
  mcp_servers: [],
  model: null,
  effort: null,
  fast: false,
  permission_mode: "auto",
  title: null,
  status: "idle",
  agent_session_id: null,
  created_at: `2026-10-18T10:00:0${id.length}Z`,
  running_tasks: 0,
  closed: false,
  last_activity_at: null,
  ...more,
});

describe("chat tabs", () => {
  test("a chat whose subagent or background task still runs after its turn shows working", () => {
    expect(chatTabStatus(chat("a"))).toBe("idle");
    expect(chatTabStatus(chat("a", { running_tasks: 1 }))).toBe("working");
    expect(chatTabStatus(chat("a", { status: "starting" }))).toBe("working");
    expect(chatTabStatus(chat("a", { status: "error" }))).toBe("error");
  });

  test("a chat waiting for the user shows it, even while its tasks run", () => {
    expect(chatTabStatus(chat("a", { status: "needs_input", running_tasks: 2 }))).toBe("needs_input");
  });

  test("the Chat view's tab shows a chat waiting for the user before one working, and nothing when all are idle", () => {
    expect(chatsIndicator([])).toBeNull();
    expect(chatsIndicator([chat("a"), chat("bb", { status: "error" })])).toBeNull();
    expect(chatsIndicator([chat("a"), chat("bb", { status: "starting" })])).toBe("working");
    expect(chatsIndicator([chat("a", { running_tasks: 1 })])).toBe("working");
    expect(chatsIndicator([chat("a", { status: "working" }), chat("bb", { status: "needs_input" })])).toBe("attention");
  });

  test("tabs go in the order they were dragged to, chats never placed after by creation", () => {
    const all = [
      chat("a", { tab_position: 2 }),
      chat("bb"),
      chat("ccc", { tab_position: 0 }),
      chat("dddd", { closed: true, tab_position: 1 }),
      chat("eeeee"),
    ];
    expect(openChats(all).map((c) => c.id)).toEqual(["ccc", "a", "bb", "eeeee"]);
    // With nothing selected, the newest chat shows, wherever its tab is.
    expect(shownChat(openChats([chat("a"), chat("bb", { tab_position: 0 })]), undefined)?.id).toBe("bb");
  });

  test("a moved tab numbers every chat of the workspace again, closed ones too", () => {
    const all = [chat("a"), chat("bb", { closed: true }), chat("ccc")];
    const placed = placedChats(all, "ccc", "a");
    expect(placed.map((c) => [c.id, c.tab_position])).toEqual([
      ["a", 1],
      ["bb", 2],
      ["ccc", 0],
    ]);
    expect(openChats(placed).map((c) => c.id)).toEqual(["ccc", "a"]);
    // Labels keep counting by creation.
    expect(chatLabel(placed[2], placed)).toBe("Chat 3");
  });

  test("an untitled chat is named by its place among every chat, closed ones included", () => {
    const all = [chat("a", { title: "Login page" }), chat("b", { closed: true }), chat("c")];
    expect(chatLabel(all[0], all)).toBe("Login page");
    expect(chatLabel(all[2], all)).toBe("Chat 3");
    expect(openChats(all).map((c) => c.id)).toEqual(["a", "c"]);
  });

  test("closed chats are listed the most recently active first", () => {
    const all = [
      chat("a", { closed: true, last_activity_at: "2026-10-18T09:00:00Z" }),
      chat("b"),
      chat("c", { closed: true, last_activity_at: "2026-10-18T11:00:00Z" }),
    ];
    expect(closedChats(all).map((c) => c.id)).toEqual(["c", "a"]);
  });

  test("an agent with a turn, a question, tasks or queued messages is working", () => {
    expect(agentBusy(chat("a"), 0)).toBe(false);
    expect(agentBusy(chat("a", { status: "working" }), 0)).toBe(true);
    expect(agentBusy(chat("a", { status: "needs_input" }), 0)).toBe(true);
    expect(agentBusy(chat("a", { status: "starting" }), 0)).toBe(true);
    expect(agentBusy(chat("a", { running_tasks: 1 }), 0)).toBe(true);
    expect(agentBusy(chat("a"), 2)).toBe(true);
    expect(agentBusy(chat("a", { status: "error" }), 0)).toBe(false);
  });

  test("an archived workspace's tabs are read-only; one being created can only be renamed", () => {
    expect(tabActions("ready")).toEqual({ rename: true, close: true, delete: true, design: true, fork: true, reason: null });
    // Moved: piloted there while it answers, design chats aside; read-only while it does not.
    expect(tabActions("ready", "vm")).toMatchObject({ rename: true, close: true, delete: true, design: false, fork: true });
    expect(tabActions("ready", "vm", false)).toMatchObject({ rename: false, close: false, delete: false, design: false, fork: false });
    expect(tabActions("ready", "vm", false).reason).toContain("does not answer");
    // A chat can start while the workspace is created: a design chat too, but no fork yet.
    expect(tabActions("creating")).toMatchObject({ rename: true, close: false, delete: false, design: true, fork: false });
    for (const state of ["archived", "archiving", "cleanup_failed"] as const) {
      const actions = tabActions(state);
      expect([actions.rename, actions.close, actions.delete, actions.design, actions.fork]).toEqual([false, false, false, false, false]);
      expect(actions.reason).toContain("read-only");
    }
  });

  test("a chat forks while its agent rests, never a design chat", () => {
    expect(forkRefusal(chat("a"))).toBeNull();
    // Background tasks and queued messages stay with the original: no reason to refuse.
    expect(forkRefusal(chat("a", { running_tasks: 2 }))).toBeNull();
    for (const status of ["working", "starting", "needs_input"] as const) {
      expect(forkRefusal(chat("a", { status }))).toContain("interrupt it first");
    }
    expect(forkRefusal(chat("a", { kind: "design" }))).toContain("design chat");
  });

  test("the tab on the right shows once one leaves, else the one on the left", () => {
    const open = [chat("a"), chat("b"), chat("c")];
    expect(neighbourTab(open, "b")).toBe("c");
    expect(neighbourTab(open, "c")).toBe("b");
    expect(neighbourTab([chat("a")], "a")).toBeNull();
    expect(neighbourTab(open, "zz")).toBeNull();
  });

  test("⇧⌘T reopens the last chat closed here, else the most recently active closed one", () => {
    const closed = [chat("x", { closed: true }), chat("y", { closed: true })];
    expect(chatToReopen(closed, ["y", "gone"])?.id).toBe("y");
    expect(chatToReopen(closed, ["y", "x"])?.id).toBe("x");
    expect(chatToReopen(closed, [])?.id).toBe("x");
    expect(chatToReopen([], ["x"])).toBeNull();
  });

  test("a deleted chat leaves the store and the selection", () => {
    const state: { chats: Record<string, ChatInfo>; selectedChat: Record<string, string> } = {
      chats: { a: chat("a"), b: chat("b") },
      selectedChat: { w: "b", v: "z" },
    };
    const next = withoutChat(state, "b");
    expect(Object.keys(next.chats)).toEqual(["a"]);
    expect(next.selectedChat).toEqual({ v: "z" });
    const other = withoutChat(state, "a");
    expect(other.selectedChat).toBe(state.selectedChat);
    expect(withoutChat(state, "nope").chats).toBe(state.chats);
  });
});
