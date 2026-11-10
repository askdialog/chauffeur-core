import { describe, expect, test } from "bun:test";
import type { ChatRow } from "./chat";
import { firstItemKey, remountLocation } from "./chatList";

const message = (key: string) => ({ kind: "assistant", key, ts: "t", depth: 0, messageId: key, text: key, streaming: false }) as unknown as ChatRow;
const group = (key: string, items: string[]) =>
  ({ kind: "activity", key: `activity-${key}`, items: items.map(message) }) as unknown as ChatRow;

describe("remountLocation", () => {
  const rows = [message("s1"), group("s2", ["s2", "s3"]), message("s4")];

  test("stays at the bottom for a reader who was there", () => {
    expect(remountLocation(rows, "s4", true)).toEqual({ index: "LAST", align: "end" });
    expect(remountLocation(rows, null, false)).toEqual({ index: "LAST", align: "end" });
  });

  test("goes back to the row of the item a reader had at the top", () => {
    expect(remountLocation(rows, "s4", false)).toEqual({ index: 2, align: "start" });
    // The item is now inside a group that grew with the earlier messages.
    expect(remountLocation(rows, "s3", false)).toEqual({ index: 1, align: "start" });
  });

  test("falls back to the bottom when the item is gone", () => {
    expect(remountLocation(rows, "s99", false)).toEqual({ index: "LAST", align: "end" });
  });

  test("names a group by its first item", () => {
    expect(firstItemKey(rows[1])).toBe("s2");
    expect(firstItemKey(rows[0])).toBe("s1");
  });
});
