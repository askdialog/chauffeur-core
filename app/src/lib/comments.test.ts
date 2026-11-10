import { describe, expect, test } from "bun:test";
import type { CanvasScreen, DesignComment } from "@chauffeur/protocol";
import type { ShownSection } from "./canvas";
import { canChange, counts, isStale, keepMessage, mergeFound, parseCaptureReply, pinsOf, placeOf, popoverAnchor, selectorsToCheck, threadsOf, withCapture } from "./comments";

const me = { name: "Alice", email: "alice@example.com" };
const bob = { name: "Bob", email: "bob@example.com" };

const comment = (id: string, more: Partial<DesignComment> = {}): DesignComment => ({
  id,
  screen: "a/hover.html",
  author: me,
  at: 1,
  text: `text ${id}`,
  ...more,
});

const comments: DesignComment[] = [
  comment("t1", { pin: { x: 48, y: 83, element: { selector: "button.cta", text: "Contacter" } } }),
  comment("r1", { reply_to: "t1", author: bob, sent: { at: 5 } }),
  comment("t2", { screen: "b/gone.html", resolved: { by: "agent", at: 3 }, sent: { at: 2 } }),
  comment("t3", { pin: { x: 10, y: 10 }, sent: { at: 2 } }),
  comment("t4", { pin: { x: 20, y: 20, element: { selector: "h1", text: "Title" } } }),
];

describe("threadsOf and counts", () => {
  test("threads are the first comments, numbered in order, with their replies", () => {
    const threads = threadsOf(comments);
    expect(threads.map((t) => [t.number, t.comment.id, t.replies.map((r) => r.id)])).toEqual([
      [1, "t1", ["r1"]],
      [2, "t2", []],
      [3, "t3", []],
      [4, "t4", []],
    ]);
  });

  test("counts open and resolved threads, and what was not sent", () => {
    expect(counts(comments)).toEqual({ open: 3, resolved: 1, unsent: 2 });
    expect(counts([])).toEqual({ open: 0, resolved: 0, unsent: 0 });
  });
});

describe("pins", () => {
  test("a screen's open threads are pins, the whole-screen one in the corner, resolved ones gone", () => {
    const threads = threadsOf(comments);
    expect(pinsOf(threads, "a/hover.html", null).map((p) => [p.thread.number, p.x, p.y, p.whole, p.stale])).toEqual([
      [1, 48, 83, false, false],
      [3, 10, 10, false, false],
      [4, 20, 20, false, false],
    ]);
    expect(pinsOf(threads, "b/gone.html", null)).toEqual([]);
    expect(pinsOf(threadsOf([comment("w", { screen: "c.html" })]), "c.html", null)[0]).toMatchObject({ x: 0, y: 0, whole: true });
  });

  test("a pin is stale once its element is no longer found", () => {
    const threads = threadsOf(comments);
    const found = { "button.cta": false, h1: true };
    expect(pinsOf(threads, "a/hover.html", found).map((p) => p.stale)).toEqual([true, false, false]);
    expect(isStale(comments[0]!, null)).toBe(false);
    expect(isStale(comments[3]!, { "button.cta": false })).toBe(false);
    expect(selectorsToCheck(threads, "a/hover.html")).toEqual(["button.cta", "h1"]);
  });
});

describe("popoverAnchor", () => {
  const [t1, , t3] = threadsOf(comments);
  test("a bubble being written opens at its point, or in the corner for the whole screen", () => {
    const draft = { file: "a/hover.html", x: 50, y: 25, whole: false, element: null };
    expect(popoverAnchor("a/hover.html", 400, 800, draft, t1)).toEqual({ x: 200, y: 200 });
    expect(popoverAnchor("a/hover.html", 400, 800, { ...draft, whole: true }, null)).toEqual({ x: 10, y: 10 });
    expect(popoverAnchor("b/other.html", 400, 800, draft, null)).toBeNull();
  });
  test("an open thread opens at its pin, on its own screen only", () => {
    expect(popoverAnchor("a/hover.html", 200, 100, null, t3)).toEqual({ x: 20, y: 10 });
    expect(popoverAnchor("b/other.html", 200, 100, null, t3)).toBeNull();
    expect(popoverAnchor("a/hover.html", 200, 100, null, null)).toBeNull();
  });
  test("a thread on the whole screen opens in its corner", () => {
    const [whole] = threadsOf([comment("w")]);
    expect(popoverAnchor("a/hover.html", 200, 100, null, whole)).toEqual({ x: 10, y: 10 });
  });
});

describe("mergeFound", () => {
  test("an element found in any frame of a version of the screen stays found", () => {
    // Commented in full screen with a modal open: the canvas's frame, as served, has no modal.
    const overlay = mergeFound(undefined, "v1", { "div.modal": true });
    expect(mergeFound(overlay, "v1", { "div.modal": false, h1: true }).selectors).toEqual({ "div.modal": true, h1: true });
  });
  test("a new version of the screen is judged afresh", () => {
    const before = mergeFound(undefined, "v1", { "div.modal": true });
    expect(mergeFound(before, "v2", { "div.modal": false })).toEqual({ src: "v2", selectors: { "div.modal": false } });
  });
});

describe("canChange and placeOf", () => {
  test("only the author changes a comment, until it is sent", () => {
    expect(canChange(comments[0]!, me)).toBe(true);
    expect(canChange(comments[0]!, bob)).toBe(false);
    expect(canChange(comments[2]!, me)).toBe(false);
  });

  test("a thread's place is its element, its position or the whole screen", () => {
    expect(placeOf(comments[0]!)).toBe("On “Contacter”");
    expect(placeOf(comments[3]!)).toBe("At 10%, 10%");
    expect(placeOf(comments[2]!)).toBe("Whole screen");
  });

  test("a markdown file's thread is on its passage, else on the whole file", () => {
    const onFile = { ...comments[2]!, screen: ".chauffeur/plans/plan.md" };
    expect(placeOf({ ...onFile, quote: "Ship it on Friday" })).toBe("On “Ship it on Friday”");
    expect(placeOf(onFile)).toBe("Whole file");
  });
});

describe("parseCaptureReply", () => {
  test("reads the element under a point, or none", () => {
    expect(parseCaptureReply({ type: "chauffeur:element", request: 3, element: { selector: "a.x", text: "Go" } })).toEqual({
      kind: "element",
      request: 3,
      element: { selector: "a.x", text: "Go" },
    });
    expect(parseCaptureReply({ type: "chauffeur:element", request: 3, element: null })).toEqual({ kind: "element", request: 3, element: null });
  });

  test("reads which selectors are found, and ignores anything else", () => {
    expect(parseCaptureReply({ type: "chauffeur:check", request: 1, found: { h1: true, "a.x": "yes" } })).toEqual({
      kind: "check",
      request: 1,
      found: { h1: true, "a.x": false },
    });
    expect(parseCaptureReply({ type: "other", request: 1 })).toBeNull();
    expect(parseCaptureReply({ type: "chauffeur:element", element: null })).toBeNull();
    expect(parseCaptureReply("x")).toBeNull();
  });

  test("the canvas asks for the capture script in the screen's URL", () => {
    expect(withCapture("http://127.0.0.1:1/s/w/c/a.html")).toBe("http://127.0.0.1:1/s/w/c/a.html?capture=1");
    expect(withCapture("http://127.0.0.1:1/s/w/c/a.html?v=2")).toBe("http://127.0.0.1:1/s/w/c/a.html?v=2&capture=1");
  });
});

describe("keepMessage", () => {
  const screen = (file: string, label?: string): CanvasScreen => ({ file, label, width: 390, height: 844, url: "u", modified_ms: 1 });
  const sections: ShownSection[] = [
    { title: "A", unsorted: false, screens: [screen("a/default.html", "Repos"), screen("a/hover.html", "Survol")] },
    { title: "B", unsorted: false, screens: [screen("b/default.html", "Repos")] },
  ];

  test("names the kept screens by section and asks to drop the rest", () => {
    expect(keepMessage("chat1", sections, ["b/default.html", "a/hover.html"])).toBe(
      [
        "Keep these screens and remove the other sections (and the other screens of these sections) from canvas.json and the folder:",
        "",
        "A:",
        "- Survol: `.chauffeur/designs/chat1/a/hover.html`",
        "",
        "B:",
        "- Repos: `.chauffeur/designs/chat1/b/default.html`",
      ].join("\n"),
    );
    expect(keepMessage("chat1", sections, [])).toBeNull();
  });
});
