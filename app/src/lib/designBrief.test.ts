import { describe, expect, test } from "bun:test";
import type { ChatInfo } from "@chauffeur/protocol";
import { briefBanner, briefFill, briefMemos } from "./designBrief";

const chat = (id: string, more: Partial<ChatInfo> = {}): ChatInfo => ({
  id,
  kind: "chat",
  workspace_id: "w",
  agent: "claude",
  mcp_servers: [],
  model: null,
  effort: null,
  fast: false,
  permission_mode: "accept_edits",
  title: null,
  status: "idle",
  agent_session_id: null,
  created_at: "2026-10-18T10:00:00Z",
  running_tasks: 0,
  closed: false,
  last_activity_at: null,
  ...more,
});

describe("the brief's banner", () => {
  const source = chat("s", { title: "Checkout page", agent: "codex" });
  const design = (brief: ChatInfo["brief"]) => chat("d", { kind: "design", source_chat: "s", brief });

  test("names the source chat while the brief is written", () => {
    expect(briefBanner(design({ state: "writing" }), [source])).toEqual({
      state: "writing",
      message: "Summarizing “Checkout page”…",
    });
  });

  test("an untitled source is named as its tab is; a source not in the list is “the chat”", () => {
    const untitled = chat("s");
    expect(briefBanner(design({ state: "writing" }), [untitled])?.message).toBe("Summarizing “Chat 1”…");
    expect(briefBanner(design({ state: "writing" }), [])?.message).toBe("Summarizing the chat…");
  });

  test("a failure says why", () => {
    expect(briefBanner(design({ state: "failed", error: "timed out after 2 minutes" }), [source])).toEqual({
      state: "failed",
      message: "The brief could not be written: timed out after 2 minutes",
    });
    expect(briefBanner(design({ state: "failed" }), [source])?.message).toBe("The brief could not be written: no reason given");
  });

  test("a written brief offers its regeneration", () => {
    expect(briefBanner(design({ state: "ready", text: "Design the checkout" }), [source])?.state).toBe("ready");
  });

  test("nothing without a brief (a blank design chat, or once its first message is sent), nor for a chat that codes", () => {
    expect(briefBanner(design(undefined), [source])).toBeNull();
    expect(briefBanner(chat("c", { brief: { state: "writing" } }), [source])).toBeNull();
  });
});

describe("filling the draft with the brief", () => {
  const ready = { state: "ready" as const, text: "Design the checkout" };

  test("waits for the brief to be written", () => {
    expect(briefFill(undefined, "", undefined)).toBeNull();
    expect(briefFill({ state: "writing" }, "", undefined)).toBeNull();
    expect(briefFill({ state: "failed", error: "no" }, "", undefined)).toBeNull();
    expect(briefFill({ state: "ready" }, "", undefined)).toBeNull();
  });

  test("fills an empty draft, whitespace included", () => {
    expect(briefFill(ready, "", undefined)).toBe("fill");
    expect(briefFill(ready, "  \n", undefined)).toBe("fill");
  });

  test("never overwrites what the user wrote: the brief is let go", () => {
    expect(briefFill(ready, "My own idea", undefined)).toBe("skip");
  });

  test("once taken, emptying the draft does not bring it back", () => {
    expect(briefFill(ready, "", { seen: ready.text, replace: false })).toBeNull();
  });

  test("the brief written after a Regenerate replaces the draft, whatever it holds", () => {
    expect(briefFill(ready, "edited brief", { seen: null, replace: true })).toBe("fill");
    // Still writing: the draft stays as it is meanwhile.
    expect(briefFill({ state: "writing" }, "edited brief", { seen: null, replace: true })).toBeNull();
  });

  test("the memo records what was taken and a Regenerate under way", () => {
    briefMemos.seen("d1", "text");
    expect(briefMemos.get("d1")).toEqual({ seen: "text", replace: false });
    briefMemos.regenerating("d1");
    expect(briefMemos.get("d1")).toEqual({ seen: null, replace: true });
    expect(briefMemos.get("other")).toBeUndefined();
  });
});
