import { describe, expect, test } from "bun:test";
import type { ChatRow } from "./chat";
import {
  markTime,
  messagePreview,
  placeTicks,
  pixelTicks,
  railShown,
  rowLayout,
  sameTicks,
  timelineMarks,
  type Viewport,
} from "./timeline";

const user = (key: string, text: string, attachments = 0): ChatRow => ({
  kind: "user",
  key,
  ts: "2026-10-13T09:00:00Z",
  depth: 0,
  text,
  queued: false,
  attachments: Array.from({ length: attachments }, (_, i) => ({ file: `f${i}`, name: `f${i}.png`, media_type: "image/png", size: 1, path: `/tmp/f${i}.png` })),
  messageId: null,
});
const answer = (key: string): ChatRow => ({
  kind: "assistant",
  key,
  ts: "2026-10-13T09:00:01Z",
  depth: 0,
  messageId: key,
  text: "Done.",
  streaming: false,
  parentToolUseId: null,
});

describe("timelineMarks", () => {
  test("marks the user messages only, with their row index", () => {
    const rows = [user("u1", "First"), answer("a1"), { kind: "activity", key: "g", items: [] } as ChatRow, user("u2", "Second")];
    expect(timelineMarks(rows).map((m) => [m.index, m.key, m.preview])).toEqual([
      [0, "u1", "First"],
      [3, "u2", "Second"],
    ]);
  });

  test("a message with only images is named by its attachments", () => {
    expect(timelineMarks([user("u", "  ", 2)])[0].preview).toBe("2 attachments");
  });

  test("the marks are the previous ones while only the agent's rows change", () => {
    const first = user("u1", "First");
    const before = timelineMarks([first, answer("a1")]);
    expect(timelineMarks([first, answer("a1"), answer("a2")], before)).toBe(before);
    const after = timelineMarks([first, answer("a1"), user("u2", "Second")], before);
    expect(after).not.toBe(before);
    expect(after.map((m) => m.key)).toEqual(["u1", "u2"]);
  });
});

describe("messagePreview", () => {
  test("puts the message on one line", () => {
    expect(messagePreview("Fix the\n\n  cart   page")).toBe("Fix the cart page");
  });

  test("cuts a long message at a word, with an ellipsis", () => {
    const preview = messagePreview("word ".repeat(60), 40);
    expect(preview.endsWith("word…")).toBe(true);
    expect(preview.length).toBeLessThanOrEqual(41);
  });
});

describe("rowLayout", () => {
  test("sums the size ranges into offsets, the last range running on", () => {
    const layout = rowLayout(
      [
        { startIndex: 0, endIndex: 1, size: 50 },
        { startIndex: 2, endIndex: 2, size: 200 },
        { startIndex: 3, endIndex: Infinity, size: 30 },
      ],
      5,
    );
    expect(layout.offsets).toEqual([0, 50, 100, 300, 330]);
    expect(layout.heights).toEqual([50, 50, 200, 30, 30]);
    expect(layout.total).toBe(360);
  });

  test("nothing measured yet: every row is flat", () => {
    expect(rowLayout([], 2)).toEqual({ offsets: [0, 0], heights: [0, 0], total: 0 });
  });
});

describe("placeTicks", () => {
  // Rows of 100px under a 12px header: the scroll content is 12 + 1000 + 16 px tall.
  const rows = [user("u0", "a"), ...Array.from({ length: 4 }, (_, i) => answer(`a${i}`)), user("u5", "b"), answer("a5"), answer("a6"), answer("a7"), user("u9", "c")];
  const layout = rowLayout([{ startIndex: 0, endIndex: Infinity, size: 100 }], rows.length);
  const marks = timelineMarks(rows);
  const at = (scrollTop: number): Viewport => ({ scrollTop, clientHeight: 300, scrollHeight: 1028, header: 12 });

  test("places each message proportionally to its offset in the whole conversation", () => {
    const ticks = placeTicks(marks, layout, at(0));
    expect(ticks.map((t) => t.top)).toEqual([12 / 1028, 512 / 1028, 912 / 1028]);
  });

  test("tells which messages are in the window", () => {
    expect(placeTicks(marks, layout, at(0)).map((t) => t.inView)).toEqual([true, false, false]);
    expect(placeTicks(marks, layout, at(400)).map((t) => t.inView)).toEqual([false, true, false]);
    // A message partly shown at the bottom of the window counts as in view.
    expect(placeTicks(marks, layout, at(620)).map((t) => t.inView)).toEqual([false, false, true]);
  });

  test("marks of rows the layout does not know yet are left out", () => {
    expect(placeTicks(marks, rowLayout([], 3), at(0)).map((t) => t.key)).toEqual(["u0"]);
  });

  test("sameTicks ignores sub-pixel moves but not a change of window", () => {
    const a = placeTicks(marks, layout, at(0));
    expect(sameTicks(a, placeTicks(marks, layout, { ...at(0), scrollHeight: 1028.2 }))).toBe(true);
    expect(sameTicks(a, placeTicks(marks, layout, at(400)))).toBe(false);
  });
});

describe("railShown", () => {
  const two = timelineMarks([user("u1", "a"), user("u2", "b")]);
  test("needs two user messages and a chat that scrolls", () => {
    expect(railShown(two, { scrollTop: 0, clientHeight: 300, scrollHeight: 900, header: 12 })).toBe(true);
    expect(railShown(two.slice(0, 1), { scrollTop: 0, clientHeight: 300, scrollHeight: 900, header: 12 })).toBe(false);
    expect(railShown(two, { scrollTop: 0, clientHeight: 300, scrollHeight: 300, header: 12 })).toBe(false);
  });
});

describe("markTime", () => {
  test("only the clock for a message of today, the day too before", () => {
    const now = new Date(2026, 9, 17, 15, 0);
    const today = markTime(new Date(2026, 9, 17, 9, 5).toISOString(), now);
    const before = markTime(new Date(2026, 9, 13, 9, 5).toISOString(), now);
    expect(today).toMatch(/05/);
    expect(before.endsWith(today)).toBe(true);
    expect(before.length).toBeGreaterThan(today.length);
    expect(markTime("not a date", now)).toBe("");
  });
});

test("a huge timeline bounds hit targets to screen pixels while preserving the current message", () => {
  const ticks = Array.from({ length: 50000 }, (_, i) => ({ index: i, key: `s${i}`, preview: "Message", ts: "t", top: i / 50000, inView: i === 25000 }));
  const rendered = pixelTicks(ticks, 800);
  expect(rendered.length).toBeLessThanOrEqual(801);
  expect(rendered.some(tick => tick.index === 25000 && tick.inView)).toBe(true);
  expect(rendered.at(-1)?.index).toBe(49999);
});
