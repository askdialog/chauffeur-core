import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import { Markdown } from "./Markdown";

// Server rendering hoists image preloads and keeps the newlines between blocks: neither shows.
const drawn = (text: string, streaming: boolean) =>
  renderToStaticMarkup(<Markdown text={text} streaming={streaming} />)
    .replace(/<link [^>]*\/>/g, "")
    .replace(/\n/g, "")
    .replace(" md-streaming", "");

test("a message streamed in pieces is drawn as the whole message", () => {
  const text = [
    "# Plan",
    "First step, with `code` and a [link](https://example.com).",
    "```ts\nconst a = 1;\n\nconst b = 2;\n```",
    "1. one\n\n2. two\n\n   still two",
    "| a | b |\n| - | - |\n| 1 | 2 |",
    "> quoted",
    "Last line",
  ].join("\n\n");
  expect(drawn(text, true)).toBe(drawn(text, false));
});

test("only a declared document becomes a card; HTML and malformed declarations stay code", () => {
  const declaration = '{"title":"Feature overview","path":".chauffeur/artifacts/feature/index.html"}';
  expect(drawn(`\`\`\`deliverable\n${declaration}\n\`\`\``, false)).toContain('data-testid="deliverable-card"');
  for (const text of [`\`\`\`json\n${declaration}\n\`\`\``, "```html\n<h1>Report</h1>\n```", "```deliverable\n{\"title\":\n```", "`deliverable`"]) {
    expect(drawn(text, false)).not.toContain('data-testid="deliverable-card"');
  }
});
