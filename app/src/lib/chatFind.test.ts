import { describe, expect, test } from "bun:test";
import type { ChatItem, ChatRow } from "./chat";
import { findInChat, findInText, findOwner, firstMatchFrom, foldText, matchLabel, searchableText, stepMatch } from "./chatFind";

const base = { ts: "2026-10-25T09:00:00Z", depth: 0 };
const user = (key: string, text: string): ChatItem => ({ ...base, kind: "user", key, text, queued: false, attachments: [], messageId: null });
const answer = (key: string, text: string, depth = 0): ChatItem => ({
  ...base,
  depth,
  kind: "assistant",
  key,
  messageId: key,
  text,
  streaming: false,
  parentToolUseId: null,
});
const thinking = (key: string, text: string): ChatItem => ({ ...base, kind: "thinking", key, messageId: key, text, streaming: false });
const tool = (key: string, summary: string): ChatItem => ({
  ...base,
  kind: "tool",
  key,
  toolUseId: key,
  name: "Bash",
  input: { command: summary },
  parentToolUseId: null,
  summary,
  result: { output: summary, isError: false, attachments: [] },
  asked: false,
});

describe("foldText", () => {
  test("folds each run of whitespace to one space, as the page lays it out", () => {
    const { folded, from } = foldText("a \n\n b");
    expect(folded).toBe("a b");
    expect(from).toEqual([0, 1, 5]);
  });

  test("drops case and accents and maps each character back to the original", () => {
    const { folded, from, to } = foldText("Élève Ça");
    expect(folded).toBe("eleve ca");
    expect(from).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    expect(to).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });

  test("an accent written as its own combining mark folds away with it", () => {
    const decomposed = "Café!";
    const { folded, from, to } = foldText(decomposed);
    expect(folded).toBe("cafe!");
    expect(from[4]).toBe(5);
    expect(to[3]).toBe(4);
  });
});

describe("findInText", () => {
  test("finds every match whatever its case and accents, as offsets into the original text", () => {
    const text = "Le café, puis un CAFE et un Café.";
    const matches = findInText(text, "cafe");
    expect(matches.map(({ start, end }) => text.slice(start, end))).toEqual(["café", "CAFE", "Café"]);
  });

  test("an accented query finds the plain spelling too", () => {
    expect(findInText("resume, résumé", "Résumé")).toHaveLength(2);
  });

  test("matches do not overlap", () => {
    expect(findInText("aaaa", "aa")).toEqual([
      { start: 0, end: 2 },
      { start: 2, end: 4 },
    ]);
  });

  test("a blank query matches nothing", () => {
    expect(findInText("some text", "")).toEqual([]);
    expect(findInText("some text", "   ")).toEqual([]);
    expect(findInText("café", "\u0301")).toEqual([]);
  });

  test("a match after a character outside the basic plane keeps its offsets", () => {
    const text = "🚀 launch";
    const [match] = findInText(text, "launch");
    expect(text.slice(match.start, match.end)).toBe("launch");
  });
});

describe("searchableText", () => {
  test("offers the user's messages and the main agent's text only", () => {
    expect(searchableText(user("u", "hello"))).toBe("hello");
    expect(searchableText(answer("a", "hi"))).toBe("hi");
    expect(searchableText(answer("s", "a subagent's text", 1))).toBeNull();
    expect(searchableText(thinking("t", "pondering"))).toBeNull();
    expect(searchableText(tool("b", "grep cart"))).toBeNull();
    expect(searchableText({ kind: "activity", key: "g", items: [tool("b", "cart")] })).toBeNull();
  });
});

describe("findInChat", () => {
  const rows: ChatRow[] = [
    user("u1", "Fix the cart page"),
    { kind: "activity", key: "g1", items: [thinking("t1", "the cart"), tool("b1", "grep cart")] },
    answer("a1", "The Cart flashed; the cart now renders once."),
    user("u2", "Thanks"),
    answer("a2", "Le panier (cart) est corrigé."),
  ];

  test("lists the matches from the top of the chat, several per row in order", () => {
    expect(findInChat(rows, "CART")).toEqual([
      { index: 0, key: "u1", occurrence: 0 },
      { index: 2, key: "a1", occurrence: 0 },
      { index: 2, key: "a1", occurrence: 1 },
      { index: 4, key: "a2", occurrence: 0 },
    ]);
  });

  test("leaves thinking and tool calls out", () => {
    expect(findInChat(rows, "grep")).toEqual([]);
  });

  test("a blank query finds nothing", () => {
    expect(findInChat(rows, " ")).toEqual([]);
  });

  test("counts in the agent's text as drawn, not in its markdown", () => {
    const drawn: ChatRow[] = [
      answer("a1", "Changed [cart.ts](app/cart.ts), **really**: run `bun test`.\n\n```ts\nconst app = 1;\n```"),
    ];
    // The link's URL and the code block's language are not shown; its code is.
    expect(findInChat(drawn, "app")).toEqual([{ index: 0, key: "a1", occurrence: 0 }]);
    expect(findInChat(drawn, "ts")).toHaveLength(1);
    expect(findInChat(drawn, "**")).toEqual([]);
    expect(findInChat(drawn, "`")).toEqual([]);
    expect(findInChat(drawn, "really: run bun test")).toHaveLength(1);
  });

  test("keeps the user's own message as typed", () => {
    expect(findInChat([user("u1", "see **this**")], "**")).toHaveLength(2);
  });
});

describe("firstMatchFrom", () => {
  const matches = [0, 2, 2, 7].map((index, i) => ({ index, key: `r${index}`, occurrence: i }));

  test("starts on the first match from the top of the view down", () => {
    expect(firstMatchFrom(matches, 0)).toBe(0);
    expect(firstMatchFrom(matches, 1)).toBe(1);
    expect(firstMatchFrom(matches, 7)).toBe(3);
  });

  test("falls back on the last match above the view when none is below", () => {
    expect(firstMatchFrom(matches, 9)).toBe(3);
  });

  test("is -1 without matches", () => {
    expect(firstMatchFrom([], 0)).toBe(-1);
  });
});

describe("stepMatch", () => {
  test("goes to the next and the previous match", () => {
    expect(stepMatch(1, 5, 1)).toBe(2);
    expect(stepMatch(1, 5, -1)).toBe(0);
  });

  test("wraps around at both ends", () => {
    expect(stepMatch(4, 5, 1)).toBe(0);
    expect(stepMatch(0, 5, -1)).toBe(4);
  });

  test("starts at an end when no match is current", () => {
    expect(stepMatch(-1, 5, 1)).toBe(0);
    expect(stepMatch(-1, 5, -1)).toBe(4);
    expect(stepMatch(7, 5, 1)).toBe(0);
  });

  test("is -1 without matches", () => {
    expect(stepMatch(0, 0, 1)).toBe(-1);
  });
});

describe("matchLabel", () => {
  test("says which match is current, or that there is none", () => {
    expect(matchLabel(1, 5, "cart")).toBe("2 of 5");
    expect(matchLabel(-1, 0, "cart")).toBe("No results");
    expect(matchLabel(-1, 0, "")).toBe("");
  });
});

describe("findOwner", () => {
  test("the chat the key was typed in finds, whichever was clicked last", () => {
    expect(findOwner([{ typedIn: false, touched: 5 }, { typedIn: true, touched: 1 }])).toBe(1);
    expect(findOwner([{ typedIn: true, touched: 0 }, { typedIn: false, touched: 3 }])).toBe(0);
  });

  test("typed outside every chat, the one last clicked or focused finds", () => {
    expect(findOwner([{ typedIn: false, touched: 4 }, { typedIn: false, touched: 2 }])).toBe(0);
  });

  test("with none touched yet, the last one shown finds: a floating chat over another", () => {
    expect(findOwner([{ typedIn: false, touched: 0 }, { typedIn: false, touched: 0 }])).toBe(1);
  });

  test("a single chat always finds, and none means no bar", () => {
    expect(findOwner([{ typedIn: false, touched: 0 }])).toBe(0);
    expect(findOwner([])).toBe(-1);
  });
});
