import { describe, expect, test } from "bun:test";
import type { Question } from "@chauffeur/protocol";
import {
  answersOf,
  choose,
  confirmOther,
  hasPreviews,
  initialState,
  moveCursor,
  needsReview,
  previewMarkdown,
  previewOf,
  rowsOf,
  typeOther,
} from "./questions";

const color: Question = {
  question: "Which colour?",
  header: "Colour",
  multi_select: false,
  options: [
    { label: "Red", description: "warm" },
    { label: "Blue", description: "cold" },
  ],
};
const sizes: Question = {
  question: "Which sizes?",
  header: "Sizes",
  multi_select: true,
  options: [
    { label: "S", description: "" },
    { label: "M", description: "" },
  ],
};

describe("answering the agent's questions", () => {
  test("one single-choice question sends as soon as an option is picked", () => {
    const questions = [color];
    expect(needsReview(questions)).toBe(false);
    const { state, send } = choose(questions, initialState(questions), 1);
    expect(send).toBe(true);
    expect(answersOf(questions, state)).toEqual({ "Which colour?": "Blue" });
  });

  test("a free answer replaces the options of a single choice", () => {
    const questions = [color];
    let state = choose(questions, initialState(questions), 2).state; // "Type something…"
    expect(state.editing).toBe(true);
    state = typeOther(state, "  Green ");
    const { state: done, send } = confirmOther(questions, state);
    expect(send).toBe(true);
    expect(answersOf(questions, done)).toEqual({ "Which colour?": "Green" });
  });

  test("several questions move to the next unanswered one, then to the review", () => {
    const questions = [color, sizes];
    expect(rowsOf(sizes).map((r) => r.kind)).toEqual(["option", "option", "other", "next"]);
    let { state, send } = choose(questions, initialState(questions), 0);
    expect(send).toBe(false);
    expect(state.tab).toBe(1);
    state = choose(questions, state, 0).state; // S
    state = choose(questions, state, 1).state; // M
    state = choose(questions, state, 0).state; // S again: unticked
    state = choose(questions, state, 3).state; // Next
    expect(state.tab).toBe(2);
    const sent = choose(questions, state, 0);
    expect(sent.send).toBe(true);
    expect(answersOf(questions, sent.state)).toEqual({ "Which colour?": "Red", "Which sizes?": "M" });
  });

  test("the review sends nothing while a question is unanswered, and can decline", () => {
    const questions = [color, sizes];
    const atReview = { ...initialState(questions), tab: 2 };
    const back = choose(questions, atReview, 0);
    expect(back.send).toBe(false);
    expect(back.state.tab).toBe(0);
    expect(choose(questions, atReview, 1).decline).toBe(true);
    expect(moveCursor(questions, atReview, 5).review).toBe(1);
  });
});

describe("option previews", () => {
  const snippets: Question = {
    question: "Which snippet?",
    header: "Snippet",
    multi_select: false,
    options: [
      { label: "Loop", description: "", preview: "```ts\nfor (const x of xs) f(x);\n```" },
      { label: "Map", description: "", preview: "   " },
      { label: "Reduce", description: "" },
    ],
  };

  test("a question shows previews when one of its options has one", () => {
    expect(hasPreviews(snippets)).toBe(true);
    // Codex's questions, and Claude's without previews: the layout is unchanged.
    expect(hasPreviews(color)).toBe(false);
  });

  test("the focused row's preview, none for a blank one, the free answer or a row past the options", () => {
    expect(previewOf(snippets, 0)).toContain("for (const x of xs)");
    expect(previewOf(snippets, 1)).toBeNull();
    expect(previewOf(snippets, 2)).toBeNull();
    expect(previewOf(snippets, 3)).toBeNull();
  });

  test("moving the cursor moves the preview", () => {
    const state = moveCursor([snippets], initialState([snippets]), 1);
    expect(previewOf(snippets, state.cursor[0])).toBeNull();
    const back = moveCursor([snippets], state, -1);
    expect(previewOf(snippets, back.cursor[0])).toContain("for (const x of xs)");
  });
});

describe("a preview as markdown", () => {
  test("markdown with fenced code, or prose, is shown as given", () => {
    const fenced = "Loop:\n```ts\nfor (const x of xs) {\n  f(x);\n}\n```";
    expect(previewMarkdown(fenced)).toBe(fenced);
    expect(previewMarkdown("**Pros**: simple\n- one\n- two")).toBe("**Pros**: simple\n- one\n- two");
  });

  test("bare code keeps its indentation in a code block, in the language guessed", () => {
    const code = "function sum(xs: number[]) {\n  let total = 0;\n  return total;\n}";
    expect(previewMarkdown(code, () => "typescript")).toBe("```typescript\n" + code + "\n```");
    expect(previewMarkdown(code)).toBe("```\n" + code + "\n```");
  });

  test("an ASCII mockup stays verbatim; backticks inside get a longer fence", () => {
    expect(previewMarkdown("┌────┐\n│ OK │\n└────┘")).toBe("```\n┌────┐\n│ OK │\n└────┘\n```");
    expect(previewMarkdown("a = 1\n  b = ```c```")).toBe("````\na = 1\n  b = ```c```\n````");
  });
});
