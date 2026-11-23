import type { Nodes } from "mdast";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import ReactMarkdown from "react-markdown";
import remarkParse from "remark-parse";
import { unified } from "unified";
import { remarkPlugins } from "./markdown";

const parser = unified().use(remarkParse).use(remarkPlugins);

/** Portable HTML without the chat's controls, file handlers or diagram UI. */
export function markdownHtml(text: string): string {
  return renderToStaticMarkup(createElement(ReactMarkdown, { remarkPlugins }, text));
}

/**
 * `markdown` with each heading, at any level and ATX or setext, written as a bold line
 * (`**Title**`), its closing `#`s dropped; the rest is left byte for byte. Slack converts pasted
 * Markdown itself and drops the blank line under a heading, not under a bold line. A heading's own
 * bold spans are unwrapped so that it never reads `****`; an empty heading is left as it is.
 */
export function headingsAsBold(markdown: string): string {
  const edits: { start: number; end: number; text: string }[] = [];
  const visit = (node: Nodes) => {
    if (node.type === "heading") {
      const start = node.position?.start.offset;
      const end = node.position?.end.offset;
      const first = node.children[0]?.position?.start.offset;
      const last = node.children.at(-1)?.position?.end.offset;
      if (start === undefined || end === undefined || first === undefined || last === undefined) return;
      let text = "";
      let from = first;
      for (const child of node.children) {
        if (child.type !== "strong") continue;
        const inner = child.children[0]?.position?.start.offset;
        const innerEnd = child.children.at(-1)?.position?.end.offset;
        const strongEnd = child.position?.end.offset;
        if (inner === undefined || innerEnd === undefined || strongEnd === undefined) continue;
        text += markdown.slice(from, child.position?.start.offset) + markdown.slice(inner, innerEnd);
        from = strongEnd;
      }
      edits.push({ start, end, text: `**${text}${markdown.slice(from, last)}**` });
    } else if ("children" in node) {
      node.children.forEach(visit);
    }
  };
  visit(parser.parse(markdown));
  let result = markdown;
  for (const { start, end, text } of edits.reverse()) result = result.slice(0, start) + text + result.slice(end);
  return result;
}

/**
 * Keep the source for text editors and Slack, which converts pasted Markdown itself, and offer rich
 * text to applications that accept it. The plain text has its headings as bold lines
 * (`headingsAsBold`), the HTML is rendered from the source as written.
 */
export async function copyMarkdown(text: string): Promise<void> {
  await writeClipboard(headingsAsBold(text), markdownHtml(text));
}

const escapes: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };

/**
 * `text` as HTML lines, each in its own `<div>`, its spaces kept: a pasted line stays as written.
 * An empty line is `<div><br></div>`, as rich editors write it.
 */
export function linesHtml(text: string): string {
  return text
    .split("\n")
    .map((line) => {
      const escaped = line.replace(/[&<>"]/g, (char) => escapes[char] ?? char).replace(/\t/g, "    ");
      const kept = escaped.replace(/ {2,}|^ /g, (spaces) => "&nbsp;".repeat(spaces.length));
      return `<div>${kept || "<br>"}</div>`;
    })
    .join("");
}

/**
 * A code block's text as written, for editors, with the same lines as HTML for rich editors. Slack
 * reads pasted plain text as Markdown: a line starting with `1.` or `-` becomes a list item and the
 * indented lines under it are glued onto it. It takes the HTML lines as they are.
 */
export async function copyCodeBlock(text: string): Promise<void> {
  await writeClipboard(text, linesHtml(text));
}

async function writeClipboard(plain: string, html: string): Promise<void> {
  if (typeof ClipboardItem !== "undefined" && navigator.clipboard?.write) {
    try {
      await navigator.clipboard.write([
        new ClipboardItem({
          "text/plain": new Blob([plain], { type: "text/plain" }),
          "text/html": new Blob([html], { type: "text/html" }),
        }),
      ]);
      return;
    } catch {
      // Some webviews expose rich copying but only allow plain text.
    }
  }
  await navigator.clipboard.writeText(plain);
}
