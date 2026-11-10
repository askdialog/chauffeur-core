import { describe, expect, test } from "bun:test";
import { markdown } from "@codemirror/lang-markdown";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { codePreview, type Span } from "./codePreview";

// `|` marks the caret (and is not part of the text); no `|` means the composer is not focused.
function preview(typed: string, selection?: Span, range?: Span) {
  const caret = typed.indexOf("|");
  const doc = typed.replace("|", "");
  const state = EditorState.create({ doc, extensions: markdown() });
  const tree = ensureSyntaxTree(state, doc.length, 5000);
  if (!tree) throw new Error("the markdown did not parse");
  const ranges = selection ? [selection] : caret >= 0 ? [{ from: caret, to: caret }] : null;
  return { doc, ...codePreview(state.doc, tree, ranges, range?.from, range?.to) };
}

const text = (doc: string, span: Span) => doc.slice(span.from, span.to);

describe("codePreview", () => {
  test("inline code is found with its backticks, however many", () => {
    const { doc, inline } = preview("run `ls` or ``a`b``");
    expect(inline.map((code) => text(doc, code))).toEqual(["`ls`", "``a`b``"]);
    expect(inline[1]?.marks.map((mark) => text(doc, mark))).toEqual(["``", "``"]);
  });

  test("inline code opens while the caret is in it or right beside it, not one character away", () => {
    expect(preview("a `ls` b|").inline[0]?.open).toBe(false);
    expect(preview("a| `ls` b").inline[0]?.open).toBe(false);
    expect(preview("a |`ls` b").inline[0]?.open).toBe(true);
    expect(preview("a `l|s` b").inline[0]?.open).toBe(true);
    expect(preview("a `ls`| b").inline[0]?.open).toBe(true);
  });

  test("a selection across code opens it", () => {
    expect(preview("a `ls` b", { from: 0, to: 8 }).inline[0]?.open).toBe(true);
  });

  test("nothing is open while the composer is not focused", () => {
    const { inline, blocks } = preview("`ls`\n```ts\nx\n```");
    expect(inline[0]?.open).toBe(false);
    expect(blocks[0]?.open).toBe(false);
  });

  test("a fenced block spans its fence lines, the opening one with its info string", () => {
    const { doc, blocks } = preview("see\n```ts title=x\nconst x = 1;\n```\nok|");
    expect(blocks).toHaveLength(1);
    const [block] = blocks;
    expect(block?.lines.map((at) => doc.slice(at, doc.indexOf("\n", at)))).toEqual(["```ts title=x", "const x = 1;", "```"]);
    expect(block?.fences.map((fence) => text(doc, fence))).toEqual(["```ts title=x", "```"]);
    expect(block?.language).toBe("ts");
    expect(block?.open).toBe(false);
  });

  test("a fenced block opens while the caret is on any of its lines, its fences included", () => {
    expect(preview("```\nx|\n```").blocks[0]?.open).toBe(true);
    expect(preview("|```\nx\n```").blocks[0]?.open).toBe(true);
    expect(preview("```\nx\n```|").blocks[0]?.open).toBe(true);
    expect(preview("```\nx\n```\n|").blocks[0]?.open).toBe(false);
  });

  test("a block being typed has no closing fence yet, and one without info string no language", () => {
    const { doc, blocks } = preview("```\nconst x|");
    expect(blocks[0]?.fences.map((fence) => text(doc, fence))).toEqual(["```"]);
    expect(blocks[0]?.lines).toHaveLength(2);
    expect(blocks[0]?.language).toBe("");
  });

  test("a block in a list keeps the list mark out of its fence", () => {
    const { doc, blocks } = preview("- ```sh\n  ls\n  ```\n|");
    expect(blocks[0]?.fences.map((fence) => text(doc, fence))).toEqual(["```sh", "```"]);
  });

  test("a long block has only its first, last and drawn lines, in order", () => {
    const doc = `\`\`\`ts\n${Array.from({ length: 20000 }, (_, i) => `line ${i}`).join("\n")}\n\`\`\``;
    const shown = { from: doc.indexOf("line 10000\n"), to: doc.indexOf("line 10002\n") + 4 };
    const [block] = preview(doc, undefined, shown).blocks;
    const lineAt = (at: number) => doc.slice(at).split("\n")[0];
    expect(block?.lines.map(lineAt)).toEqual(["```ts", "line 10000", "line 10001", "line 10002", "```"]);
  });

  test("a block drawn from its top or to its bottom does not repeat its fence lines", () => {
    const doc = "```\na\nb\n```";
    expect(preview(doc, undefined, { from: 0, to: doc.length }).blocks[0]?.lines).toHaveLength(4);
    expect(preview(doc, undefined, { from: 0, to: 1 }).blocks[0]?.lines).toHaveLength(2);
    expect(preview("```", undefined, { from: 0, to: 3 }).blocks[0]?.lines).toHaveLength(1);
  });
});
