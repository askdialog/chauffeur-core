import { expect, test } from "bun:test";
import { DOUBLE_ESC_MS, isDoubleEsc, NO_RECALL, recall, recallable } from "./promptHistory";

const history = recallable(["one", " two ", "two", "", "three"]);

test("recallable drops blanks and immediate repeats", () => {
  expect(history).toEqual(["one", "two", "three"]);
});

test("up walks back from the newest and stops at the oldest", () => {
  const first = recall(history, NO_RECALL, "draft", "up")!;
  expect(first.text).toBe("three");
  const second = recall(history, first.state, "three", "up")!;
  expect(second.text).toBe("two");
  const third = recall(history, second.state, "two", "up")!;
  expect(third.text).toBe("one");
  expect(recall(history, third.state, "one", "up")).toBeNull();
});

test("down walks forward and ends on the draft", () => {
  const up = recall(history, recall(history, NO_RECALL, "my draft", "up")!.state, "three", "up")!;
  const down = recall(history, up.state, "two", "down")!;
  expect(down.text).toBe("three");
  const back = recall(history, down.state, "three", "down")!;
  expect(back.text).toBe("my draft");
  expect(recall(history, back.state, "my draft", "down")).toBeNull();
});

test("nothing to recall from an empty history", () => {
  expect(recall([], NO_RECALL, "", "up")).toBeNull();
});

test("a second Esc soon after the first is a double Esc", () => {
  expect(isDoubleEsc(null, 1000)).toBe(false);
  expect(isDoubleEsc(1000, 1000 + DOUBLE_ESC_MS)).toBe(true);
  expect(isDoubleEsc(1000, 1001 + DOUBLE_ESC_MS)).toBe(false);
});
