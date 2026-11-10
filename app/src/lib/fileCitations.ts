import type { PhrasingContent, Root, RootContent } from "mdast";

/** Render Codex's file references through the same links as ordinary Markdown files. */
export function remarkFileCitations() {
  return (tree: Root) => {
    function visit(node: Root | RootContent) {
      if (node.type === "link" || node.type === "linkReference" || !("children" in node)) return;
      for (let i = 0; i < node.children.length; i++) {
        let child = node.children[i];
        // GFM splits a quoted web citation into text, an automatic link and more text.
        if (child.type === "text" && /:codex-file-citation\{[^{}]*path="$/.test(child.value)) {
          const link = node.children[i + 1];
          const tail = node.children[i + 2];
          if (link?.type === "link" && link.children.length === 1 && link.children[0].type === "text"
            && link.children[0].value === link.url && tail?.type === "text" && tail.value.startsWith('"')) {
            child = { type: "text", value: child.value + link.url + tail.value };
            node.children.splice(i, 3, child);
          }
        }
        if (child.type !== "text") {
          visit(child);
          continue;
        }
        const parts: PhrasingContent[] = [];
        let end = 0;
        for (const match of child.value.matchAll(/:codex-file-citation\{((?:[^"{}]|"[^"\n]*")*)\}/g)) {
          const path = /(?:^|\s)path="([^"\n]+)"(?:\s|$)/.exec(match[1])?.[1];
          if (!path || (!path.startsWith("/") && !/^https?:\/\/[^\s]+$/i.test(path))) continue;
          parts.push({ type: "text", value: child.value.slice(end, match.index) });
          parts.push({ type: "link", url: /^https?:/i.test(path) ? path : encodeURI(path), children: [{ type: "text", value: /^https?:/i.test(path) ? path : path.split("/").at(-1) || path }] });
          end = match.index + match[0].length;
        }
        if (!end) continue;
        parts.push({ type: "text", value: child.value.slice(end) });
        node.children.splice(i, 1, ...parts);
        i += parts.length - 1;
      }
    }
    visit(tree);
  };
}
