// What the chat composer previews of the code typed, as Obsidian's live preview does: `inline code`
// as a chip without its backticks, a ``` fenced block as a card without its fence lines. The marks
// of the code the caret is on show again, to be edited as typed. It only decides what shows: the
// text stays the markdown typed, and ComposerEditor draws it.

import type { syntaxTree } from "@codemirror/language";
import type { Text } from "@codemirror/state";

export type Span = { from: number; to: number };

export type InlineCode = Span & {
  /** Its opening and closing backticks. */
  marks: [Span, Span];
  /** Whether the caret is in it or beside it: its backticks show. */
  open: boolean;
};

export type CodeBlock = {
  /** The start of its first and last lines, and of each line between that is in the range asked. */
  lines: number[];
  /** Its opening fence, info string included, and its closing one: a block being typed has none yet. */
  fences: Span[];
  /** The first word of its info string (`ts`), or "". */
  language: string;
  /** Whether the caret is on one of its lines: its fence lines show. */
  open: boolean;
};

export type CodePreview = { inline: InlineCode[]; blocks: CodeBlock[] };

/**
 * The code of `doc` between `from` and `to` (the lines the editor draws), and which of it is open. `selection` is null while the
 * composer is not focused: no code is open then.
 */
export function codePreview(
  doc: Text,
  tree: ReturnType<typeof syntaxTree>,
  selection: readonly Span[] | null,
  from = 0,
  to = doc.length,
): CodePreview {
  // A caret right before or after the code opens it too, so that arrows never step over a hidden mark.
  const touched = (span: Span) => selection?.some((range) => range.from <= span.to && range.to >= span.from) ?? false;
  const preview: CodePreview = { inline: [], blocks: [] };
  tree.iterate({
    from,
    to,
    enter: (node) => {
      if (node.name === "InlineCode") {
        const [open, close] = node.node.getChildren("CodeMark");
        if (open && close) {
          const span = { from: node.from, to: node.to };
          preview.inline.push({ ...span, marks: [spanOf(open), spanOf(close)], open: touched(span) });
        }
        return false;
      }
      if (node.name !== "FencedCode") return;
      const [opening, closing] = node.node.getChildren("CodeMark");
      const first = doc.lineAt(node.from);
      const last = doc.lineAt(node.to);
      const fences = opening ? [{ from: opening.from, to: doc.lineAt(opening.from).to }] : [];
      if (closing && doc.lineAt(closing.from).number > first.number) fences.push({ from: closing.from, to: last.to });
      const info = node.node.getChild("CodeInfo");
      // Its lines in the range only, besides the first and last that close the card: a block
      // pasted long would otherwise take one decoration per line at each keystroke.
      const lines = [first.from];
      const top = Math.max(first.number + 1, doc.lineAt(Math.max(from, first.from)).number);
      const bottom = Math.min(last.number - 1, doc.lineAt(Math.min(to, last.to)).number);
      for (let number = top; number <= bottom; number++) lines.push(doc.line(number).from);
      if (last.number > first.number) lines.push(last.from);
      preview.blocks.push({
        lines,
        fences,
        language: info ? (/\S*/.exec(doc.sliceString(info.from, info.to))?.[0] ?? "") : "",
        open: touched({ from: first.from, to: last.to }),
      });
      return false;
    },
  });
  return preview;
}

const spanOf = ({ from, to }: Span): Span => ({ from, to });
