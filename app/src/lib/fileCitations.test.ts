import { expect, test } from "bun:test";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import { remarkPlugins } from "./markdown";

const citation = ':codex-file-citation{path="/workspace/.chauffeur/attachments/report.pdf" purpose="source"}';
const render = (text: string) => renderToStaticMarkup(createElement(ReactMarkdown, { remarkPlugins: remarkPlugins }, text));

test("a Codex PDF citation becomes a file link with its filename", () => {
  expect(render(`Source : ${citation}`)).toBe('<p>Source : <a href="/workspace/.chauffeur/attachments/report.pdf">report.pdf</a></p>');
});

test("multiple citations preserve surrounding text and encode spaces in paths", () => {
  expect(render(`${citation} and :codex-file-citation{purpose="source" path="/workspace/my report.pdf"}.`))
    .toContain('</a> and <a href="/workspace/my%20report.pdf">my report.pdf</a>.');
});

test("code examples and existing links keep citation syntax literal", () => {
  expect(render(`\`${citation}\``)).toContain("<code>:codex-file-citation{");
  expect(render(`\`\`\`text\n${citation}\n\`\`\``)).toContain('<pre><code class="language-text">:codex-file-citation{');
  expect(render(`[${citation}](https://example.com)`)).not.toContain('href="/workspace');
});

test("incomplete or invalid citations remain visible without creating links", () => {
  for (const text of [citation.slice(0, -1), ':codex-file-citation{purpose="source"}', ':codex-file-citation{path="javascript:alert(1)"}']) {
    expect(render(text)).not.toContain("<a ");
  }
});

test("Google Slides citations become external links and preserve their descriptions", () => {
  const urls = [
    "https://docs.google.com/presentation/d/16rMledanKDnUl8deHG67BWRml0CLRIdUgDG-5fU-Q-E/edit",
    "https://docs.google.com/presentation/d/1tFZitgKiktApHO_VJDrES8C6Ux7A95dUE1irXeg50Qc/edit",
  ];
  const html = render(urls.map((url) => `:codex-file-citation{path="${url}" purpose="source"} — slides AI Bar`).join("\n"));
  for (const url of urls) expect(html).toContain(`<a href="${url}">${url}</a> — slides AI Bar`);
  expect(html).not.toContain(":codex-file-citation");
});
