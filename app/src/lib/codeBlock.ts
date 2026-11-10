import type { Element, ElementContent } from "hast";

/** The text of a markdown code block (`<pre><code>`), as written, without the newline that ends it. */
export function codeBlockText(pre: Element | undefined): string {
  if (!pre) return "";
  return textOf(pre.children).replace(/\n$/, "");
}

function textOf(nodes: ElementContent[]): string {
  return nodes.map((node) => (node.type === "text" ? node.value : node.type === "element" ? textOf(node.children) : "")).join("");
}
