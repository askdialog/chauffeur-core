// The composer's editor tells the user's edits (typing, paste, undo) from a text set from outside:
// emptied after a send, a design brief, a suggestion taken with Tab, a refused shell command put
// back. Only the user's reach `onChange` and the undo history, so ⌘Z never brings back a message sent.

import { type EditorState, Transaction, type TransactionSpec } from "@codemirror/state";

/** The transaction that shows `value` set from outside, the caret at its end; null when it shows already. */
export function externalText(state: EditorState, value: string): TransactionSpec | null {
  if (state.doc.toString() === value) return null;
  return {
    changes: { from: 0, to: state.doc.length, insert: value },
    selection: { anchor: value.length },
    annotations: Transaction.addToHistory.of(false),
  };
}

/** Whether these transactions hold a change of the user's, rather than only a text set from outside. */
export function editedByUser(transactions: readonly Transaction[]): boolean {
  return transactions.some((tr) => tr.docChanged && tr.annotation(Transaction.userEvent) !== undefined);
}
