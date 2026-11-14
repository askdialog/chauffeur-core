import { expect, test } from "bun:test";
import { afterChatScroll, chatRowPosition } from "./chatScroll";
const end = { top: 900, height: 1000, viewport: 100 };
const follow = { following: true, awaitingUserScroll: false };
const paused = { following: false, awaitingUserScroll: false };

test("scrolling into history without a wheel stops following", () => {
  expect(afterChatScroll(follow, end, { ...end, top: 600 }, false)).toEqual(paused);
});
test("new output and row measurement compensation preserve the end request", () => {
  expect(afterChatScroll(follow, end, { ...end, height: 1200 }, false)).toEqual(follow);
  expect(afterChatScroll(follow, { ...end, height: 1200 }, { ...end, height: 1000, top: 700 }, false)).toEqual(follow);
});
test("queued bottom events cannot undo an upward scroll request", () => {
  const waiting = { following: false, awaitingUserScroll: true };
  expect(afterChatScroll(waiting, end, end, false)).toEqual(waiting);
  expect(afterChatScroll(waiting, end, { ...end, top: 600 }, false)).toEqual(paused);
});
test("returning to the end resumes live following", () => {
  expect(afterChatScroll(paused, { ...end, top: 600 }, end, false)).toEqual(follow);
});
test("a navigation through the end does not resume following mid-jump", () => {
  expect(afterChatScroll(follow, end, end, true)).toEqual(paused);
});

test("a distant row uses measured ranges and estimates gaps without walking all messages", () => {
  const ranges = [{ startIndex: 0, endIndex: 1, size: 30 }, { startIndex: 4, endIndex: Infinity, size: 90 }];
  expect(chatRowPosition(ranges, 3)).toEqual({ top: 120, height: 60 });
  expect(chatRowPosition(ranges, 100000)).toEqual({ top: 180 + 99996 * 90, height: 90 });
  expect(chatRowPosition([], 50000)).toEqual({ top: 3000000, height: 60 });
});

test("scrolling up while output grows still stops following", () => {
  expect(afterChatScroll(follow, end, { ...end, top: 600, height: 1100 }, false)).toEqual(paused);
});

test("finishing a cancelled jump cannot erase a pending upward wheel", () => {
  const waiting = { following: false, awaitingUserScroll: true };
  expect(afterChatScroll(waiting, end, end, true)).toEqual(waiting);
});

test("measurement compensation cannot cancel Latest, while an upward user input can", () => {
  const displaced = { ...end, top: 600 };
  expect(afterChatScroll(follow, end, displaced, false, true)).toEqual(follow);
  expect(afterChatScroll({ following: true, awaitingUserScroll: true }, end, displaced, false, true)).toEqual(paused);
});
