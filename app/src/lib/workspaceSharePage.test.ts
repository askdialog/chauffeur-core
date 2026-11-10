import { describe, expect, test } from "bun:test";
import type { ChatEvent, EventEnvelope, WorkspaceShareSnapshot } from "@chauffeur/protocol";
import { chatTitle, guestChatStatus, guestSocketUrl, guestState, loaded, reduceGuest, seeThread, sharePageOf, startLoading, unreadByChat } from "./workspaceSharePage";

const snapshot = (more: Partial<WorkspaceShareSnapshot> = {}): WorkspaceShareSnapshot => ({
  id: "abcdef0123456789",
  title: "Billing",
  owner: { name: "Alice", email: "alice@example.com" },
  me: { name: "Bob", email: "bob@example.com" },
  role: "suggest",
  chats: [{ id: "c1", title: "Architecture", kind: "chat", agent: "claude", status: "idle", created_at: "2026-10-29T10:00:00Z" }],
  notes: [{ path: "plan.md", text: "# Plan", modified_ms: 1 }],
  suggestions: [],
  comments: [],
  people: [],
  unread_mentions: [],
  viewers: [],
  online: true,
  ...more,
});

const said = (seq: number | null, text: string, chat = "c1"): EventEnvelope => ({
  seq,
  ts: "2026-10-29T10:00:00Z",
  event: { scope: "chat", workspace_id: "w1", chat_id: chat, event: { type: "assistant_text", message_id: `m${seq ?? text}`, text, parent_tool_use_id: null } as ChatEvent },
});

const texts = (state: ReturnType<typeof guestState>, chat = "c1") =>
  (state.views[chat]?.items ?? []).flatMap((item) => (item.kind === "assistant" ? [item.text] : []));

describe("the page's address", () => {
  test("names a design's share or a workspace's", () => {
    expect(sharePageOf("/s/ABCDEF0123456789")).toEqual({ kind: "design", id: "abcdef0123456789" });
    expect(sharePageOf("/w/abcdef0123456789/")).toEqual({ kind: "workspace", id: "abcdef0123456789" });
    expect(sharePageOf("/x/abcdef0123456789")).toBeNull();
    expect(sharePageOf("/w/not-hex")).toBeNull();
  });

  test("the guest socket is on the page's host, secure when the page is", () => {
    expect(guestSocketUrl({ protocol: "https:", host: "design.askdialog.ai" }, "ab12")).toBe("wss://design.askdialog.ai/v1/workspace-shares/ab12/guest");
    expect(guestSocketUrl({ protocol: "http:", host: "127.0.0.1:8090" }, "ab12")).toBe("ws://127.0.0.1:8090/v1/workspace-shares/ab12/guest");
  });
});

describe("a chat as a guest reads it", () => {
  test("a question or a permission waits for the owner", () => {
    expect(guestChatStatus("needs_input", "Alice")).toEqual({ label: "Waiting for Alice", tone: "attention" });
    expect(guestChatStatus("working", "Alice").label).toBe("Working");
    expect(guestChatStatus("closed", "Alice").label).toBe("Idle");
  });

  test("a chat without a title is named for its kind", () => {
    expect(chatTitle({ title: null, kind: "design" })).toBe("Design");
    expect(chatTitle({ title: "  ", kind: "chat" })).toBe("Chat");
    expect(chatTitle({ title: "Architecture", kind: "chat" })).toBe("Architecture");
  });
});

describe("the page's state", () => {
  test("events that come while a chat is read go on top of it, once", () => {
    const { state, generation } = startLoading(guestState(snapshot()), "c1");
    const meanwhile = reduceGuest(state, { type: "events", chat: "c1", events: [said(2, "second"), said(3, "third")] });
    const read = loaded(meanwhile, "c1", generation, [said(1, "first"), said(2, "second")]);
    expect(texts(read)).toEqual(["first", "second", "third"]);
    const live = reduceGuest(read, { type: "events", chat: "c1", events: [said(4, "fourth")] });
    expect(texts(live)).toEqual(["first", "second", "third", "fourth"]);
  });

  test("a chat's history replaced makes a read under way stale, and the view goes", () => {
    const { state, generation } = startLoading(guestState(snapshot()), "c1");
    const replaced = reduceGuest(state, { type: "history", chat: "c1" });
    expect(loaded(replaced, "c1", generation, [said(1, "old")]).views.c1).toBeUndefined();
    const again = startLoading(replaced, "c1");
    expect(texts(loaded(again.state, "c1", again.generation, [said(1, "new")]))).toEqual(["new"]);
  });

  test("events of a chat nobody opened are left for its read", () => {
    const state = reduceGuest(guestState(snapshot()), { type: "events", chat: "c1", events: [said(1, "x")] });
    expect(state.views.c1).toBeUndefined();
  });

  test("a chat gone from the list takes its view with it", () => {
    const { state, generation } = startLoading(guestState(snapshot()), "c1");
    const read = loaded(state, "c1", generation, [said(1, "x")]);
    const gone = reduceGuest(read, { type: "chats", chats: [] });
    expect(gone.views.c1).toBeUndefined();
    expect(gone.snapshot.chats).toEqual([]);
  });

  test("notes change by path and stay in order", () => {
    const state = reduceGuest(guestState(snapshot()), {
      type: "notes",
      changed: [
        { path: "adr/001.md", text: "# ADR", modified_ms: 2 },
        { path: "plan.md", text: "# Plan v2", modified_ms: 3 },
      ],
      removed: [],
    });
    expect(state.snapshot.notes.map((n) => [n.path, n.text])).toEqual([
      ["adr/001.md", "# ADR"],
      ["plan.md", "# Plan v2"],
    ]);
    expect(reduceGuest(state, { type: "notes", changed: [], removed: ["plan.md"] }).snapshot.notes.map((n) => n.path)).toEqual(["adr/001.md"]);
  });

  test("the comments come whole: one deleted leaves the page", () => {
    const comment = { id: "k1", chat: "c1", message: "m1", quote: "a", author: { name: "Bob", email: "bob@example.com" }, text: "Why?", at: 1 };
    const state = reduceGuest(guestState(snapshot()), { type: "comments", comments: [comment] });
    expect(state.snapshot.comments).toEqual([comment]);
    expect(reduceGuest(state, { type: "comments", comments: [] }).snapshot.comments).toEqual([]);
  });

  test("a comment that names the reader is unread until its thread is opened", () => {
    const alice = { name: "Alice", email: "alice@example.com" };
    const first = { id: "k1", chat: "c1", message: "m1", quote: "a", author: alice, text: "@Bob why?", at: 1, mentions: [{ email: "bob@example.com", name: "Bob" }] };
    const reply = { id: "r1", chat: "c1", message: "m1", quote: "", author: alice, text: "@Bob again", at: 2, reply_to: "k1", mentions: [{ email: "bob@example.com", name: "Bob" }] };
    const other = { id: "k2", chat: "c2", message: "m2", quote: "b", author: alice, text: "@Bob there", at: 3, mentions: [{ email: "bob@example.com", name: "Bob" }] };
    let state = reduceGuest(guestState(snapshot()), { type: "comments", comments: [first] });
    state = reduceGuest(state, { type: "comments", comments: [first, reply, other] });
    expect(state.snapshot.unread_mentions).toEqual(["k1", "r1", "k2"]);
    expect([...unreadByChat(state.snapshot)]).toEqual([
      ["c1", ["k1"]],
      ["c2", ["k2"]],
    ]);
    state = seeThread(state, "k1");
    expect(state.snapshot.unread_mentions).toEqual(["k2"]);
    // Deleted: no longer unread.
    expect(reduceGuest(state, { type: "comments", comments: [first, reply] }).snapshot.unread_mentions).toEqual([]);
  });

  test("the owner's Mac going away keeps when it last pushed", () => {
    const state = reduceGuest(guestState(snapshot({ last_push_at: 5 })), { type: "live", online: false });
    expect([state.snapshot.online, state.snapshot.last_push_at]).toEqual([false, 5]);
  });
});
