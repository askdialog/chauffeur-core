import { describe, expect, test } from "bun:test";
import type { ShareComment } from "@chauffeur/protocol";
import type { ChatItem } from "./chat";
import { blockOf, commentNotice, newComments, placeComments, QUOTE_MAX, quoteOf } from "./shareComments";

const comment = (id: string, more: Partial<ShareComment> = {}): ShareComment => ({
  id,
  chat: "c1",
  message: "m1",
  quote: "a monolith",
  author: { name: "Bob", email: "bob@example.com" },
  text: "Why not two services?",
  at: 10,
  ...more,
});

const block = (key: string, messageId: string, text: string): ChatItem => ({
  kind: "assistant",
  key,
  ts: "2026-11-03T10:00:00Z",
  depth: 0,
  messageId,
  text,
  streaming: false,
  parentToolUseId: null,
});

describe("placeComments", () => {
  test("a comment shows under the block of its message that holds the passage, its markdown aside", () => {
    const items = [block("s1", "m1", "Keep **a monolith**, or split"), block("s2", "m1", "Then the queue"), block("s3", "m2", "a monolith")];
    const placed = placeComments(items, [comment("k1"), comment("k2", { quote: "nowhere" })]);
    expect(placed.get("s1")?.map((t) => t.comment.id)).toEqual(["k1"]);
    // A passage no block holds: under the message's last block.
    expect(placed.get("s2")?.map((t) => t.comment.id)).toEqual(["k2"]);
    expect(placed.has("s3")).toBe(false);
  });

  test("comments on one block come oldest first, and those on a message not shown are left out", () => {
    const items = [block("s1", "m1", "Keep a monolith")];
    const placed = placeComments(items, [comment("late", { at: 20 }), comment("early", { at: 1 }), comment("gone", { message: "m9" })]);
    expect(placed.get("s1")?.map((t) => t.comment.id)).toEqual(["early", "late"]);
    expect(placed.size).toBe(1);
  });

  test("replies follow their thread's first comment, oldest first, and a reply whose thread is gone is left out", () => {
    const items = [block("s1", "m1", "Keep a monolith"), block("s2", "m2", "Then the queue")];
    const placed = placeComments(items, [
      comment("k1"),
      comment("r2", { reply_to: "k1", quote: "", at: 30 }),
      comment("r1", { reply_to: "k1", quote: "", at: 20 }),
      comment("orphan", { reply_to: "gone", quote: "", message: "m2" }),
    ]);
    const threads = placed.get("s1") ?? [];
    expect(threads.map((t) => [t.comment.id, t.replies.map((r) => r.id)])).toEqual([["k1", ["r1", "r2"]]]);
    expect(placed.has("s2")).toBe(false);
    expect(blockOf(placed, "k1")).toBe("s1");
    expect(blockOf(placed, "r1")).toBeNull();
  });
});

test("a selection is quoted on one line, cut to what the service takes", () => {
  expect(quoteOf("  Keep\n\n a   monolith ")).toBe("Keep a monolith");
  expect(quoteOf("x".repeat(QUOTE_MAX + 10)).length).toBe(QUOTE_MAX);
});

test("only comments not known before are told, with their passage", () => {
  expect(newComments([comment("k1")], [comment("k1"), comment("k2")]).map((c) => c.id)).toEqual(["k2"]);
  expect(newComments(undefined, [comment("k1")]).map((c) => c.id)).toEqual(["k1"]);
  expect(commentNotice(comment("k1"), "Architecture")).toBe("Bob comments in Architecture on “a monolith”: Why not two services?");
  expect(commentNotice(comment("r1", { reply_to: "k1", quote: "", text: "Agreed" }), "Architecture")).toBe("Bob replies in Architecture: Agreed");
});

test("the owner is not told of their own comments, nor of those that mention them (the mention tells it)", () => {
  const mine = comment("k2", { author: { name: "Alice", email: "alice@example.com" } });
  const naming = comment("k3", { mentions: [{ email: "alice@example.com", name: "Alice" }] });
  expect(newComments([], [mine, naming, comment("k4")], "alice@example.com").map((c) => c.id)).toEqual(["k4"]);
});
