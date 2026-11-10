import { expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import { markdownText, remarkPlugins } from "./markdown";
import { markdownHtml } from "./copyMarkdown";

const comment = '::code-comment{title="[P2] Bound Markdown rendering" body="Parse **external content** safely. See https://example.com/security and `{limit: 64}`." file="/workspace/my app/Markdown.tsx" start=14 end=18 priority=2 confidence=0.95}';
const render = (text: string) => renderToStaticMarkup(createElement(ReactMarkdown, { remarkPlugins }, text));

test("a review comment shows its title, Markdown body and a link to its line range", () => {
  const html = render(comment);
  expect(html).toContain('<blockquote class="md-code-comment">');
  expect(html).toContain("<strong>[P2] Bound Markdown rendering</strong>");
  expect(html).toContain("<strong>external content</strong>");
  expect(html).toContain('<a href="https://example.com/security">');
  expect(html).toContain("<code>{limit: 64}</code>");
  expect(html).toContain('href="/workspace/my%20app/Markdown.tsx#L14"');
  expect(html).toContain("Markdown.tsx:14–18</a>");
  expect(html).not.toContain("confidence=");
  expect(html).not.toContain("::code-comment");
});

test("adjacent comments preserve the text before and after them", () => {
  const html = render(`Before\n${comment}\n${comment}\nAfter`);
  expect(html.match(/class="md-code-comment"/g)).toHaveLength(2);
  expect(html).toStartWith("<p>Before</p>");
  expect(html).toEndWith("<p>After</p>");
});

test("multiline attributes and escaped quotes preserve the description", () => {
  const html = render('::code-comment{title="A \\"quoted\\" title"\nbody="First line\n\nSecond line with {braces}." start=3}');
  expect(html).toContain("A &quot;quoted&quot; title");
  expect(html).toContain("<p>First line</p>");
  expect(html).toContain("<p>Second line with {braces}.</p>");
});

test("a priority outside the title is shown once, including priority zero", () => {
  const html = render('::code-comment{title="Fix this" body="Details" priority=0}');
  expect(html).toContain("<strong>[P0] Fix this</strong>");
  expect(render(comment)).not.toContain("[P2] [P2]");
});

test("a second multiline comment and reference links in its body stay complete", () => {
  const html = render(`${comment}\n::code-comment{title="Second" body="Read [the guide][guide].\n\n[guide]: https://example.com/guide"}`);
  expect(html.match(/class="md-code-comment"/g)).toHaveLength(2);
  expect(html).toContain('<a href="https://example.com/guide">the guide</a>');
  expect(html).not.toContain("::code-comment");
});

test("incomplete and malformed comments remain visible while code examples stay literal", () => {
  for (const text of [
    comment.slice(0, -1),
    '::code-comment{title="Missing body"}',
    '::code-comment{title="Title" body="Body" broken}',
    `\`${comment}\``,
    `\`example\n${comment}\nend\``,
    `\`\`\`text\n${comment}\n\`\`\``,
    `[${comment}](https://example.com)`,
  ]) {
    expect(render(text)).not.toContain('class="md-code-comment"');
    expect(render(text)).toContain("::code-comment");
  }
});

test("unsafe file schemes and invalid lines cannot become file locations", () => {
  expect(render('::code-comment{title="Title" body="Body" file="javascript:alert(1)"}')).not.toContain("<a ");
  for (const start of ["0", "-1", "NaN", "999999999999999999999"]) {
    const html = render(`::code-comment{title="Title" body="Body" file="src/app.ts" start=${start}}`);
    expect(html).toContain('href="src/app.ts"');
    expect(html).not.toContain("#L");
  }
});

test("search and rich copy include the readable finding without directive metadata", () => {
  expect(markdownText(comment)).toContain("Bound Markdown rendering");
  expect(markdownText(comment)).toContain("external content");
  expect(markdownText(comment)).not.toContain("confidence=");
  expect(markdownHtml(comment)).toBe(render(comment));
});
