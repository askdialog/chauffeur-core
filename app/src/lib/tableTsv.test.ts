import { expect, test } from "bun:test";
import type { Element } from "hast";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";
import { remarkPlugins } from "./markdown";
import { tableTsv } from "./tableTsv";

function table(markdown: string): Element {
  const processor = unified().use(remarkParse).use(remarkPlugins).use(remarkRehype);
  const root = processor.runSync(processor.parse(markdown));
  const node = root.children.find((child) => child.type === "element" && child.tagName === "table");
  if (!node || node.type !== "element") throw new Error("no table");
  return node;
}
test("table copying uses tabs and newlines without Markdown separators or emphasis markers", () => {
  expect(tableTsv(table("| Name | Status |\n|---|---|\n| **Chauffeur** | *OK* |"))).toBe("Name\tStatus\nChauffeur\tOK");
});
test("table copying preserves empty cells, link labels and escaped pipes", () => {
  expect(tableTsv(table("| A | B | C |\n|---|---|---|\n| [docs](https://example.com) | | a\\|b |"))).toBe("A\tB\tC\ndocs\t\ta|b");
});
test("spreadsheet quoting preserves quotes, tabs and multiline cell text", () => {
  const node = table("| A | B |\n|---|---|\n| x | y |");
  const body = node.children.find((child) => child.type === "element" && child.tagName === "tbody");
  if (!body || body.type !== "element") throw new Error("no body");
  const row = body.children.find((child) => child.type === "element");
  if (!row || row.type !== "element") throw new Error("no row");
  const cell = row.children.find((child) => child.type === "element");
  if (!cell || cell.type !== "element") throw new Error("no cell");
  cell.children = [{ type: "text", value: 'say "hi"\tthen' }, { type: "element", tagName: "br", properties: {}, children: [] }, { type: "text", value: "again" }];
  expect(tableTsv(node)).toBe('A\tB\n"say ""hi""\tthen\nagain"\ty');
});
