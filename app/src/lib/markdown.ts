// The agent's markdown as components/Markdown.tsx renders it: its remark plugins, and the text a
// reader sees once its syntax is gone (for find in a chat, lib/chatFind.ts).

import type { Nodes } from "hast";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { type PluggableList, unified } from "unified";
import { remarkFileCitations } from "./fileCitations";
import { remarkCodeComments } from "./codeComments";

// Agents write single newlines meaning line breaks (lists of lines, poems, logs).
export const remarkPlugins: PluggableList = [remarkCodeComments, remarkGfm, remarkBreaks, remarkFileCitations];

// As react-markdown builds its tree: raw HTML is kept, and shown as text.
const processor = unified().use(remarkParse).use(remarkPlugins).use(remarkRehype, { allowDangerousHtml: true });

// React drops the whitespace between a table's rows and cells (hast-util-to-jsx-runtime).
const TABLE = new Set(["table", "thead", "tbody", "tfoot", "tr"]);

const texts = new Map<string, string>();

/**
 * The text of `markdown` as drawn: a link's text without its URL, no `**`, backticks nor fences, a
 * code block's language left out, raw HTML as written. A ```mermaid block is drawn as a diagram, so
 * it gives nothing, nor does an image. Blocks are separated by whitespace as in the page.
 */
export function markdownText(markdown: string): string {
  const cached = texts.get(markdown);
  if (cached !== undefined) return cached;
  let text = "";
  const walk = (node: Nodes, inTable: boolean) => {
    if (node.type === "text" || node.type === "raw") {
      if (!inTable || node.value.trim()) text += node.value;
      return;
    }
    if (node.type !== "root" && node.type !== "element") return;
    if (node.type === "element" && isMermaid(node.properties.className)) return;
    const table = node.type === "element" && TABLE.has(node.tagName);
    for (const child of node.children) walk(child, table);
  };
  walk(processor.runSync(processor.parse(markdown), { value: markdown }), false);
  // Long chats are searched at every keystroke: a message is parsed once (a streaming one at each
  // step, hence the bound).
  if (texts.size >= 2000) texts.clear();
  texts.set(markdown, text);
  return text;
}

function isMermaid(className: unknown): boolean {
  return Array.isArray(className) && className.includes("language-mermaid");
}

// Constructs that tie blocks across a blank line: link and footnote definitions, the HTML blocks
// that only end at their closing tag, and Codex's review directives (lib/codeComments.ts).
const SPANS_BLANK_LINES = /^[ \t]{0,3}(?:\[[^\]\n]+\]:|<(?:script|pre|style|textarea|!--|\?|![A-Za-z]|!\[CDATA\[))|::code-comment\{/im;
// Block quote and list markers before a fence opened inside them.
const CONTAINERS = /^(?:[ \t]*(?:>|(?:[-*+]|\d{1,9}[.)])(?=[ \t])))*[ \t]*/;
const LIST_ITEM = /^(?:[-*+]|\d{1,9}[.)])(?:[ \t]|$)/;

type Fence = { char: string; length: number; contained: boolean };

function fenceOpening(line: string): Fence | null {
  const prefix = CONTAINERS.exec(line)?.[0] ?? "";
  const match = /^(`{3,}|~{3,})(.*)$/.exec(line.slice(prefix.length));
  if (!match || (match[1][0] === "`" && match[2].includes("`"))) return null;
  return { char: match[1][0], length: match[1].length, contained: prefix !== "" };
}

function closes(fence: Fence, line: string): boolean {
  const rest = fence.contained ? line.slice(CONTAINERS.exec(line)?.[0].length ?? 0) : /^ {0,3}(.*)$/.exec(line)?.[1];
  const match = rest === undefined ? null : /^(`{3,}|~{3,})[ \t]*$/.exec(rest);
  return !!match && match[1][0] === fence.char && match[1].length >= fence.length;
}

/**
 * Cuts `markdown` into pieces that render, one after the other, as the whole does: at a blank line
 * outside code fences followed by an unindented line that does not start a list item, so that no
 * list, list item, indented code or fence is cut. Text with a construct that ties blocks across
 * blank lines (a link definition, a footnote, an HTML block) stays whole. A streamed message
 * renders its finished pieces once and parses only the last one again.
 */
export function markdownBlocks(markdown: string): string[] {
  if (SPANS_BLANK_LINES.test(markdown)) return [markdown];
  const blocks: string[] = [];
  let start = 0;
  let fence: Fence | null = null;
  let blank = false;
  let offset = 0;
  for (const raw of markdown.split("\n")) {
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
    if (fence) {
      if (closes(fence, line)) fence = null;
    } else if (line.trim() === "") {
      blank = true;
    } else {
      if (blank && offset > start && !/^[ \t]/.test(line) && !LIST_ITEM.test(line)) {
        blocks.push(markdown.slice(start, offset));
        start = offset;
      }
      blank = false;
      fence = fenceOpening(line);
    }
    offset += raw.length + 1;
  }
  blocks.push(markdown.slice(start));
  return blocks;
}
