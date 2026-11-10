import type { Element, ElementContent } from "hast";

function cellText(node: ElementContent): string {
  if (node.type === "text") return node.value;
  if (node.type !== "element") return "";
  if (node.tagName === "br") return "\n";
  if (node.tagName === "img") return String(node.properties.alt ?? "");
  return node.children.map(cellText).join("");
}

/** Spreadsheet-style text: only cells, with rendered text and quoted multiline values. */
export function tableTsv(table: Element): string {
  const rows: string[][] = [];
  const walk = (node: Element) => {
    if (node.tagName === "tr") {
      rows.push(node.children.flatMap((child) =>
        child.type === "element" && (child.tagName === "th" || child.tagName === "td")
          ? [cellText(child)] : [],
      ));
      return;
    }
    for (const child of node.children) if (child.type === "element") walk(child);
  };
  walk(table);
  const width = Math.max(0, ...rows.map((row) => row.length));
  return rows.map((row) => Array.from({ length: width }, (_, index) => {
    const cell = row[index] ?? "";
    return /[\t\r\n"]/.test(cell) ? `"${cell.replaceAll('"', '""')}"` : cell;
  }).join("\t")).join("\n");
}
