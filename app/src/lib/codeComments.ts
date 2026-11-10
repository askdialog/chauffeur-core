import type { Blockquote, Root, RootContent } from "mdast";
import remarkBreaks from "remark-breaks";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import { unified } from "unified";

const parser = unified().use(remarkParse).use(remarkGfm).use(remarkBreaks);
const directive = /(?:^|\n)[ \t]*::code-comment\{((?:[^"{}]|"(?:\\.|[^"\\])*")*)\}[ \t]*(?=\n|$)/g;

function blocks(text: string): Blockquote["children"] {
  const tree = parser.parse(text);
  parser.runSync(tree);
  return tree.children.filter((node): node is Blockquote["children"][number] =>
    node.type === "paragraph" || node.type === "heading" || node.type === "blockquote"
    || node.type === "list" || node.type === "code" || node.type === "html"
    || node.type === "thematicBreak" || node.type === "table"
    || node.type === "definition" || node.type === "footnoteDefinition",
  );
}

/** Codex review directives become ordinary Markdown blocks, including for copy and search. */
export function remarkCodeComments() {
  return (tree: Root, file: { value: unknown }) => {
    const source = String(file.value);
    const literals: { start: number; end: number }[] = [];
    function protect(node: Root | RootContent) {
      if (node.type === "inlineCode" || node.type === "code" || node.type === "link" || node.type === "linkReference" || node.type === "html") {
        const start = node.position?.start.offset;
        const end = node.position?.end.offset;
        if (start !== undefined && end !== undefined) literals.push({ start, end });
      } else if ("children" in node) {
        node.children.forEach(protect);
      }
    }
    protect(tree);
    function visit(node: Root | RootContent) {
      if (!("children" in node) || node.type === "link" || node.type === "linkReference") return;
      for (let i = 0; i < node.children.length; i++) {
        const child = node.children[i];
        if (child.type !== "paragraph") {
          visit(child);
          continue;
        }
        const start = child.position?.start.offset;
        let end = child.position?.end.offset;
        if (start === undefined || end === undefined) continue;
        if (!source.slice(start, end).includes("::code-comment{")) continue;
        let count = 1;
        // A blank line inside a quoted body can make Markdown split one directive into siblings.
        for (const candidate of source.slice(start).matchAll(directive)) {
          if (candidate.index >= end - start) break;
          const directiveEnd = start + candidate.index + candidate[0].length;
          while (end < directiveEnd && i + count < node.children.length) {
            end = node.children[i + count].position?.end.offset ?? end;
            count++;
          }
        }
        const text = source.slice(start, end);
        const replacement: RootContent[] = [];
        let offset = 0;
        // Quoted attributes can contain braces, URLs, escaped quotes and line breaks.
        for (const match of text.matchAll(directive)) {
          const location = start + match.index + match[0].indexOf("::code-comment");
          if (literals.some((range) => location >= range.start && location < range.end)) continue;
          const attributes = readAttributes(match[1]);
          if (!attributes?.title || !attributes.body) continue;
          replacement.push(...blocks(text.slice(offset, match.index)));
          const priority = attributes.priority;
          const title = priority && /^[0-3]$/.test(priority) && !/^\[P[0-3]\]/.test(attributes.title)
            ? `[P${priority}] ${attributes.title}` : attributes.title;
          const children: Blockquote["children"] = [
            { type: "paragraph", children: [{ type: "strong", children: [{ type: "text", value: title }] }] },
          ];
          children.push(...blocks(attributes.body));
          const path = attributes.file;
          if (path && !/^[a-z][a-z\d+.-]*:/i.test(path)) {
            const line = positiveInteger(attributes.start);
            const endLine = positiveInteger(attributes.end);
            const range = line ? `:${line}${endLine && endLine > line ? `–${endLine}` : ""}` : "";
            children.push({ type: "paragraph", children: [{
              type: "link",
              url: encodeURI(path).replace(/#/g, "%23").replace(/\?/g, "%3F") + (line ? `#L${line}` : ""),
              title: path + range,
              children: [{ type: "text", value: (path.split("/").at(-1) || path) + range }],
            }] });
          }
          replacement.push({ type: "blockquote", data: { hProperties: { className: ["md-code-comment"] } }, children });
          offset = match.index + match[0].length;
        }
        if (!offset) continue;
        replacement.push(...blocks(text.slice(offset)));
        node.children.splice(i, count, ...replacement);
        i += replacement.length - 1;
      }
    }
    visit(tree);
  };
}

function positiveInteger(value: string | undefined): number | null {
  if (!value || !/^\d+$/.test(value)) return null;
  const number = Number(value);
  return Number.isSafeInteger(number) && number > 0 ? number : null;
}

function readAttributes(text: string): Record<string, string> | null {
  const attributes: Record<string, string> = {};
  const pattern = /\s*([\w-]+)=("(?:\\.|[^"\\])*"|[^\s"{}]+)\s*/gy;
  let offset = 0;
  for (const match of text.matchAll(pattern)) {
    const value = match[2];
    attributes[match[1]] = value.startsWith('"') ? value.slice(1, -1).replace(/\\(["\\])/g, "$1") : value;
    offset = match.index + match[0].length;
  }
  return offset === text.length ? attributes : null;
}
