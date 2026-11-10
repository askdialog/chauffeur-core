import { describe, expect, test } from "bun:test";
import { history, undo } from "@codemirror/commands";
import { EditorState, type Transaction, type TransactionSpec } from "@codemirror/state";
import { editedByUser, externalText } from "./composerText";

// A composer's state and the transactions it went through, as the editor would apply them. Each
// change is its own undo step, as when the user pauses between them.
function composer(doc = "") {
  let state = EditorState.create({ doc, extensions: history({ newGroupDelay: 0 }) });
  const applied: Transaction[] = [];
  const apply = (spec: TransactionSpec) => {
    const tr = state.update(spec);
    applied.push(tr);
    state = tr.state;
  };
  return {
    type: (text: string) => apply({ changes: { from: state.doc.length, insert: text }, userEvent: "input.type" }),
    set: (value: string) => {
      const spec = externalText(state, value);
      if (spec) apply(spec);
    },
    undo: () => undo({ state, dispatch: (tr) => (applied.push(tr), (state = tr.state)) }),
    text: () => state.doc.toString(),
    caret: () => state.selection.main.head,
    applied,
  };
}

describe("composer text", () => {
  test("undo after a send does not bring the message back", () => {
    const box = composer();
    box.type("ship it");
    box.set("");
    box.undo();
    expect(box.text()).toBe("");
  });

  test("a text set from outside is not undone, the user's edits after it are", () => {
    const box = composer();
    box.set("The brief");
    box.type(", shorter");
    box.undo();
    expect(box.text()).toBe("The brief");
    box.undo();
    expect(box.text()).toBe("The brief");
  });

  test("a text set from outside puts the caret at its end", () => {
    const box = composer("draft");
    box.set("The suggestion");
    expect(box.caret()).toBe("The suggestion".length);
  });

  test("the text already shown is not set again", () => {
    const box = composer("same");
    box.set("same");
    expect(box.applied).toHaveLength(0);
  });

  test("only the user's edits and undo count as changed by the user", () => {
    const box = composer();
    box.set("from outside");
    expect(editedByUser(box.applied)).toBe(false);
    box.type("!");
    expect(editedByUser(box.applied.slice(-1))).toBe(true);
    box.undo();
    expect(editedByUser(box.applied.slice(-1))).toBe(true);
  });
});
