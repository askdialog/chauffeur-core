import { describe, expect, test } from "bun:test";
import type { ChatInfo, Suggestion, WorkspaceShareState } from "@chauffeur/protocol";
import {
  addGuests,
  canDeleteSuggestion,
  chatIdsOf,
  guestsDiffer,
  newPending,
  pendingCount,
  pendingSuggestions,
  selectionOf,
  selectionParams,
  selectionsDiffer,
  setRole,
  shareIndicator,
  suggestionNotice,
  suggestionStateLabel,
  toggleChat,
} from "./workspaceShare";

const suggestion = (id: string, more: Partial<Suggestion> = {}): Suggestion => ({
  id,
  chat: "c1",
  author: { name: "Bob", email: "bob@example.com" },
  text: "Split it per repo",
  at: 10,
  state: "pending",
  ...more,
});

const state = (more: Partial<NonNullable<WorkspaceShareState["share"]>> = {}): WorkspaceShareState => ({
  workspace: "w1",
  configured: true,
  email: "alice@example.com",
  connection: { state: "online" },
  past_reviewers: [],
  share: {
    id: "s1",
    url: "https://design.askdialog.ai/w/s1",
    title: "Billing",
    owner: { name: "Alice", email: "alice@example.com" },
    guests: [
      { email: "bob@example.com", role: "suggest" },
      { email: "carol@example.com", role: "viewer" },
    ],
    people: [],
    team: false,
    viewers: [],
    online: true,
    stopped: false,
    ...more,
  },
});

describe("the guests in the dialog", () => {
  test("someone added is a viewer, and named once", () => {
    const guests = addGuests([{ email: "bob@example.com", role: "suggest" }], ["bob@example.com", "carol@example.com"]);
    expect(guests).toEqual([
      { email: "bob@example.com", role: "suggest" },
      { email: "carol@example.com", role: "viewer" },
    ]);
  });

  test("a role changed, or someone added or removed, is a change; their order is not", () => {
    const guests = state().share!.guests;
    expect(guestsDiffer(guests, [...guests].reverse())).toBe(false);
    expect(guestsDiffer(guests, setRole(guests, "carol@example.com", "suggest"))).toBe(true);
    expect(guestsDiffer(guests, guests.slice(1))).toBe(true);
  });
});

describe("the sidebar's word on a shared workspace", () => {
  test("names how many are on it, who can suggest and who is looking", () => {
    expect(shareIndicator(state())).toEqual({ viewers: 0, title: "Shared with 2 colleagues, 1 can suggest: nobody looking now" });
    expect(shareIndicator(state({ viewers: ["bob@example.com"], team: true }))?.title).toBe(
      "Shared with 2 colleagues and the team, 1 can suggest: bob@example.com is looking",
    );
  });

  test("says nothing of a workspace not shared, or whose share stopped", () => {
    expect(shareIndicator(undefined)).toBeNull();
    expect(shareIndicator(state({ stopped: true }))).toBeNull();
  });
});

describe("the suggestions as the owner meets them", () => {
  test("the pending ones wait above the composer, oldest first", () => {
    const list = [suggestion("b", { at: 20 }), suggestion("sent", { state: "sent" }), suggestion("a", { at: 5 })];
    expect(pendingSuggestions(list).map((s) => s.id)).toEqual(["a", "b"]);
    expect(pendingCount({ c1: list, c2: [suggestion("c")] }, ["c1", "c2", "c3"])).toBe(3);
  });

  test("only a pending suggestion not seen before is told", () => {
    const before = [suggestion("a")];
    const after = [suggestion("a"), suggestion("b"), suggestion("c", { state: "dismissed" })];
    expect(newPending(before, after).map((s) => s.id)).toEqual(["b"]);
    expect(newPending(undefined, [suggestion("a")]).map((s) => s.id)).toEqual(["a"]);
  });

  test("a guest takes back their own suggestion while it waits or once dismissed, never a sent one", () => {
    const bob = "bob@example.com";
    expect(canDeleteSuggestion(suggestion("a"), bob)).toBe(true);
    expect(canDeleteSuggestion(suggestion("a", { state: "dismissed" }), bob)).toBe(true);
    expect(canDeleteSuggestion(suggestion("a", { state: "sent" }), bob)).toBe(false);
    expect(canDeleteSuggestion(suggestion("a"), "carol@example.com")).toBe(false);
  });

  test("a suggestion's fate and its notice read as sentences", () => {
    expect(suggestionStateLabel("sent_edited")).toBe("sent, edited");
    expect(suggestionNotice(suggestion("a"), "Architecture")).toBe("Bob suggests to Architecture: Split it per repo");
    expect(suggestionNotice(suggestion("a"), null)).toBe("Bob suggests to a chat: Split it per repo");
  });
});

describe("chatIdsOf", () => {
  const chat = (id: string, workspace_id: string): ChatInfo => ({
    id,
    kind: "chat",
    workspace_id,
    agent: "claude",
    mcp_servers: [],
    model: null,
    effort: null,
    fast: false,
    permission_mode: "auto",
    title: null,
    status: "idle",
    agent_session_id: null,
    created_at: "2026-10-18T10:00:00Z",
    running_tasks: 0,
    closed: false,
    last_activity_at: null,
  });

  test("lists a workspace's chats, the same list while the chats map is the same", () => {
    const chats = { a: chat("a", "w1"), b: chat("b", "w2"), c: chat("c", "w1") };
    const first = chatIdsOf(chats, "w1");
    expect(first).toEqual(["a", "c"]);
    expect(chatIdsOf(chats, "w2")).toEqual(["b"]);
    expect(chatIdsOf(chats, "w1")).toBe(first);
    expect(chatIdsOf(chats, "none")).toEqual([]);
    expect(chatIdsOf({ ...chats, d: chat("d", "w1") }, "w1")).toEqual(["a", "c", "d"]);
  });
});

describe("the chats a share holds", () => {
  test("a share of all chats starts as all, a selection without its closed chats", () => {
    expect(selectionOf(undefined, ["c1"])).toBeNull();
    expect(selectionOf(null, ["c1"])).toBeNull();
    expect(selectionOf(["c1", "c2"], ["c1", "c3"])).toEqual(["c1"]);
  });

  test("selections differ by their chats, not their order", () => {
    expect(selectionsDiffer(null, null)).toBe(false);
    expect(selectionsDiffer(null, ["c1"])).toBe(true);
    expect(selectionsDiffer(["c1", "c2"], ["c2", "c1"])).toBe(false);
    expect(selectionsDiffer(["c1"], ["c1", "c2"])).toBe(true);
  });

  test("checking a chat adds it once, unchecking takes it out", () => {
    expect(toggleChat(["c1"], "c2", true)).toEqual(["c1", "c2"]);
    expect(toggleChat(["c1", "c2"], "c2", true)).toEqual(["c1", "c2"]);
    expect(toggleChat(["c1", "c2"], "c1", false)).toEqual(["c2"]);
  });

  test("all chats is asked for as such, a selection by its ids", () => {
    expect(selectionParams(null)).toEqual({ all_chats: true });
    expect(selectionParams(["c1"])).toEqual({ chat_ids: ["c1"] });
  });
});
