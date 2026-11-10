import { describe, expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import { foldText } from "./chatFind";
import { markdownBlocks, markdownText, remarkPlugins } from "./markdown";

/** The text a page shows for some HTML: its tags gone, its entities read. */
const pageText = (html: string) =>
  html
    .replace(/<[^>]*>/g, "")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#x27;/g, "'")
    .replace(/&amp;/g, "&");
const drawn = (markdown: string) => pageText(renderToStaticMarkup(createElement(ReactMarkdown, { remarkPlugins }, markdown)));

describe("markdownText", () => {
  test("a link reads as its text, without its URL", () => {
    expect(markdownText("See [cart.ts](app/cart.ts) and <https://example.com>")).toBe("See cart.ts and https://example.com");
  });

  test("emphasis, inline code and a code block's fences and language leave no syntax", () => {
    expect(markdownText("**bold**, _italic_, ~~gone~~ and `code`")).toBe("bold, italic, gone and code");
    expect(markdownText("```ts\nconst a = 1;\n```")).toBe("const a = 1;\n");
  });

  test("a mermaid diagram and an image give nothing, raw HTML reads as written", () => {
    expect(markdownText("```mermaid\ngraph TD\n```\n\nafter")).toBe("\nafter");
    expect(markdownText("![a cart](cart.png) <b>x</b>")).toBe(" <b>x</b>");
  });

  test("Codex's file citations read as the file's name, as they are drawn", () => {
    expect(markdownText('In :codex-file-citation{path="/repo/app/cart.ts" line=3}.')).toBe("In cart.ts.");
  });

  test("the text is the one the page shows, whatever the markdown", () => {
    const samples = [
      "See [cart.ts](app/cart.ts), **bold** and `code`\nnext line",
      "# Title\n\n> quoted *text*\n\n1. one\n2. two\n   - nested",
      "- [ ] todo\n- [x] done\n\n---\n\nfoot[^1]\n\n[^1]: a note",
      "| a | b |\n|---|---|\n| [x](y) | `z` |",
      "```rust\nfn main() {}\n```\n\ntext after\n\n    indented code",
      "<div>raw</div>\n\nA & B < C",
    ];
    for (const markdown of samples) expect(foldText(markdownText(markdown)).folded).toBe(foldText(drawn(markdown)).folded);
  });
});

describe("markdownBlocks", () => {
  // React drops nothing between blocks but the newlines markdown puts there.
  const html = (markdown: string) => renderToStaticMarkup(createElement(ReactMarkdown, { remarkPlugins }, markdown)).replace(/>\n+</g, "><");
  const piecewise = (markdown: string) => markdownBlocks(markdown).map(html).join("");

  test("cuts at blank lines between paragraphs, headings and tables", () => {
    const text = "# Title\n\nFirst line\nsecond line\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\nLast";
    expect(markdownBlocks(text)).toEqual(["# Title\n\n", "First line\nsecond line\n\n", "| a | b |\n| - | - |\n| 1 | 2 |\n\n", "Last"]);
    expect(piecewise(text)).toBe(html(text));
  });

  test("a blank line inside a backtick or tilde fence does not cut it, even with longer or nested fences", () => {
    const text = "Before\n\n```ts\nconst a = 1;\n\nconst b = 2;\n```\n\n~~~~\n~~~\n\nstill code\n~~~~\n\nAfter";
    expect(markdownBlocks(text)).toEqual([
      "Before\n\n",
      "```ts\nconst a = 1;\n\nconst b = 2;\n```\n\n",
      "~~~~\n~~~\n\nstill code\n~~~~\n\n",
      "After",
    ]);
    expect(piecewise(text)).toBe(html(text));
  });

  test("a fence still open while the message streams keeps the rest of the text", () => {
    expect(markdownBlocks("Intro\n\n```\na\n\nb")).toEqual(["Intro\n\n", "```\na\n\nb"]);
  });

  test("a fence opened in a list item is followed to its end", () => {
    const text = "- step\n  ```sh\n  make\n\n  make test\n  ```\n\nDone";
    expect(markdownBlocks(text)).toEqual(["- step\n  ```sh\n  make\n\n  make test\n  ```\n\n", "Done"]);
    expect(piecewise(text)).toBe(html(text));
  });

  test("a loose list, its indented paragraphs and its numbering stay in one piece", () => {
    const text = "1. one\n\n2. two\n\n   more of two\n\n3. three\n\n- a\n\n  - nested\n\nAfter the list";
    expect(markdownBlocks(text)).toEqual(["1. one\n\n2. two\n\n   more of two\n\n3. three\n\n- a\n\n  - nested\n\n", "After the list"]);
    expect(piecewise(text)).toBe(html(text));
  });

  test("indented code keeps its blank lines", () => {
    const text = "Code:\n\n    a\n\n    b\n\nEnd";
    expect(piecewise(text)).toBe(html(text));
    expect(markdownBlocks(text)).toEqual(["Code:\n\n    a\n\n    b\n\n", "End"]);
  });

  test("link and footnote definitions, HTML blocks and review directives keep the text whole", () => {
    for (const text of [
      "See [the docs][docs].\n\n[docs]: https://example.com",
      "A claim[^1].\n\n[^1]: The source.",
      "<!--\nnote\n\nmore\n-->\n\nText",
      "<pre>\na\n\nb\n</pre>\n\nText",
      'Intro\n\n::code-comment{title="x" body="a\n\nb"}',
    ]) {
      expect(markdownBlocks(text)).toEqual([text]);
    }
  });
});
