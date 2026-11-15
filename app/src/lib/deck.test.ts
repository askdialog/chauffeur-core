import { expect, test } from "bun:test";
import type { DeckSlide } from "@chauffeur/protocol";
import { fitScale, moved, parseReaderMessage, slidePath, step } from "./deck";

const slide = (file: string, skip = false): DeckSlide => ({ file, title: file, notes: "", skip, version: "1" });

test("only well-formed messages of a slide's script are read", () => {
  expect(parseReaderMessage({ type: "chauffeur:edit", tag: "h1", index: 0, before: "A", html: "B", overflow: true }))
    .toEqual({ kind: "edit", tag: "h1", index: 0, before: "A", html: "B", overflow: true });
  expect(parseReaderMessage({ type: "chauffeur:key", key: "ArrowRight" })).toEqual({ kind: "key", key: "ArrowRight" });
  for (const bad of [null, "x", { type: "chauffeur:edit", tag: "<h1>", index: 0, before: "", html: "" }, { type: "chauffeur:edit", tag: "p", index: -1, before: "", html: "" }, { type: "other" }]) {
    expect(parseReaderMessage(bad)).toBeNull();
  }
});

test("a slide lies beside its deck.json", () => {
  expect(slidePath(".chauffeur/artifacts/q3/deck.json", "02-plan.html")).toBe(".chauffeur/artifacts/q3/02-plan.html");
});

test("a slide is scaled down to fit its box, never up", () => {
  expect(fitScale(960, 1000, 1920, 1080)).toBe(0.5);
  expect(fitScale(4000, 4000, 1920, 1080)).toBe(1);
  expect(fitScale(0, 100, 1920, 1080)).toBe(0);
});

test("a dragged slide lands before the one it is dropped on", () => {
  expect(moved(["a", "b", "c", "d"], 0, 2)).toEqual(["b", "a", "c", "d"]);
  expect(moved(["a", "b", "c", "d"], 3, 0)).toEqual(["d", "a", "b", "c"]);
  expect(moved(["a", "b", "c"], 0, 3)).toEqual(["b", "c", "a"]);
});

test("presenting steps over skipped slides, reading does not", () => {
  const slides = [slide("1"), slide("2", true), slide("3")];
  expect(step(slides, 0, "ArrowRight", true)).toBe(2);
  expect(step(slides, 0, "ArrowRight", false)).toBe(1);
  expect(step(slides, 2, "ArrowLeft", true)).toBe(0);
  expect(step(slides, 2, "ArrowRight", true)).toBe(2);
  expect(step(slides, 1, "End", true)).toBe(2);
  expect(step(slides, 0, "x", true)).toBeNull();
});
