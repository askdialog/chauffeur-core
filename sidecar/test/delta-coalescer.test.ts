import { describe, expect, test } from "bun:test";
import { DeltaCoalescer } from "../src/delta-coalescer.ts";
import type { ChatEvent } from "../src/protocol.ts";

function coalescer(windowMs = 10_000) {
  const sent: Array<[string, ChatEvent]> = [];
  return { sent, deltas: new DeltaCoalescer((chatId, event) => sent.push([chatId, event]), windowMs) };
}

describe("coalescing streamed deltas", () => {
  test("consecutive deltas of one message go out as one event with their text joined", () => {
    const { sent, deltas } = coalescer();
    deltas.push("c1", { type: "text_delta", message_id: "m:0", text: "Hel", parent_tool_use_id: "t1" });
    deltas.push("c1", { type: "text_delta", message_id: "m:0", text: "lo", parent_tool_use_id: "t1" });
    deltas.push("c1", { type: "text_delta", message_id: "m:0", text: " world", parent_tool_use_id: "t1" });
    expect(sent).toEqual([]);
    deltas.flush("c1");
    expect(sent).toEqual([["c1", { type: "text_delta", message_id: "m:0", text: "Hello world", parent_tool_use_id: "t1" }]]);
  });

  test("another event of the chat goes out after the deltas held before it", () => {
    const { sent, deltas } = coalescer();
    deltas.push("c1", { type: "thinking_delta", message_id: "m:0", text: "a" });
    deltas.push("c1", { type: "thinking_delta", message_id: "m:0", text: "b" });
    deltas.push("c1", { type: "user_message_delivered", message_id: "u1" });
    expect(sent.map(([, event]) => event.type)).toEqual(["thinking_delta", "user_message_delivered"]);
    expect(sent[0]?.[1]).toEqual({ type: "thinking_delta", message_id: "m:0", text: "ab" });
  });

  test("deltas of another message, kind or writer are never merged", () => {
    const { sent, deltas } = coalescer();
    deltas.push("c1", { type: "text_delta", message_id: "m:0", text: "a" });
    deltas.push("c1", { type: "text_delta", message_id: "m:1", text: "b" });
    deltas.push("c1", { type: "thinking_delta", message_id: "m:1", text: "c" });
    deltas.push("c1", { type: "thinking_delta", message_id: "m:1", text: "d", parent_tool_use_id: "t1" });
    deltas.flush("c1");
    expect(sent.map(([, event]) => (event as { text: string }).text)).toEqual(["a", "b", "c", "d"]);
  });

  test("each chat holds its own deltas", () => {
    const { sent, deltas } = coalescer();
    deltas.push("c1", { type: "text_delta", message_id: "m:0", text: "one" });
    deltas.push("c2", { type: "turn_started" });
    deltas.push("c2", { type: "text_delta", message_id: "m:0", text: "two" });
    expect(sent).toEqual([["c2", { type: "turn_started" }]]);
    deltas.flush("c1");
    deltas.flush("c2");
    expect(sent.slice(1)).toEqual([
      ["c1", { type: "text_delta", message_id: "m:0", text: "one" }],
      ["c2", { type: "text_delta", message_id: "m:0", text: "two" }],
    ]);
  });

  test("held deltas go out on their own once the window passes", async () => {
    const { sent, deltas } = coalescer(5);
    deltas.push("c1", { type: "text_delta", message_id: "m:0", text: "a" });
    deltas.push("c1", { type: "text_delta", message_id: "m:0", text: "b" });
    await Bun.sleep(30);
    expect(sent).toEqual([["c1", { type: "text_delta", message_id: "m:0", text: "ab" }]]);
    deltas.flush("c1");
    expect(sent).toHaveLength(1);
  });
});
