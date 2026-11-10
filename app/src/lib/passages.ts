// A markdown file's comments (story 57) are on a passage of the rendered file, kept as its text.
// These find the passage again in the rendered text, whitespace ignored (a selection across blocks
// and the rendered blocks do not space the same), to highlight it; a comment whose passage the
// file no longer holds is outdated.

/** Where `quote` is in `text`, whitespace ignored: its first offset and the one past its end; null when absent. */
export function findPassage(text: string, quote: string): { start: number; end: number } | null {
  const wanted = quote.replace(/\s+/g, "");
  if (!wanted) return null;
  const positions: number[] = [];
  let compact = "";
  for (let i = 0; i < text.length; i += 1) {
    const char = text.charAt(i);
    if (/\s/.test(char)) continue;
    compact += char;
    positions.push(i);
  }
  const at = compact.indexOf(wanted);
  const start = positions[at];
  const last = positions[at + wanted.length - 1];
  if (at < 0 || start === undefined || last === undefined) return null;
  return { start, end: last + 1 };
}

/** The text of `root`, as its text nodes hold it, and where each node starts in it. */
function textOf(root: Node): { text: string; nodes: { node: Text; start: number }[] } {
  const walker = (root.ownerDocument ?? document).createTreeWalker(root, 4 /* NodeFilter.SHOW_TEXT */);
  const nodes: { node: Text; start: number }[] = [];
  let text = "";
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    nodes.push({ node: node as Text, start: text.length });
    text += node.nodeValue ?? "";
  }
  return { text, nodes };
}

/** The range of `quote` in the text under `root`; null when it is not there. */
export function passageRange(root: Node, quote: string): Range | null {
  const { text, nodes } = textOf(root);
  const found = findPassage(text, quote);
  if (!found) return null;
  // The node holding an offset: the last starting at or before it (before it, for an end).
  const at = (offset: number, end: boolean) => {
    const holder = [...nodes].reverse().find((n) => (end ? n.start < offset : n.start <= offset));
    return holder ? { node: holder.node, offset: offset - holder.start } : null;
  };
  const start = at(found.start, false);
  const end = at(found.end, true);
  if (!start || !end) return null;
  const range = (root.ownerDocument ?? document).createRange();
  range.setStart(start.node, start.offset);
  range.setEnd(end.node, end.offset);
  return range;
}

/** The text a selection's range covers, as the rendered file holds it (no list markers, no added line breaks). */
export function rangeText(range: Range): string {
  return range.cloneContents().textContent ?? "";
}
