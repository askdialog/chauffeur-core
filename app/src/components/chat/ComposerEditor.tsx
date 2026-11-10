// The chat composer's text field: CodeMirror in markdown mode, so that `inline code` and ``` fenced
// blocks are highlighted while the user types, a fence's info string naming the language, and
// previewed as chips and cards while the caret is elsewhere. The text stays the markdown typed,
// sent as it is: the preview only hides its marks, no language is picked elsewhere.

import { history, historyKeymap, insertNewline, standardKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { HighlightStyle, syntaxHighlighting, syntaxTree } from "@codemirror/language";
import { Compartment, EditorState, type Extension, Prec, type Range } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  keymap,
  placeholder as placeholderWidget,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { type Ref, useEffect, useImperativeHandle, useRef } from "react";
import { fenceLanguage } from "@/lib/codeLanguages";
import { codePreview } from "@/lib/codePreview";
import { editedByUser, externalText } from "@/lib/composerText";

export type ComposerEditorHandle = { focus: () => void };

export function ComposerEditor({
  ref,
  value,
  disabled = false,
  placeholder,
  onChange,
  onKeyDown,
  onPaste,
}: {
  ref?: Ref<ComposerEditorHandle>;
  /** Replaces the text when it is not the one shown, the caret at its end, out of the undo history. */
  value: string;
  disabled?: boolean;
  placeholder: string;
  /** The text the user changed (typing, paste, undo); not called for a new `value`. */
  onChange: (text: string) => void;
  /** Runs before the editor's own keys: a key it prevents never reaches the editor. */
  onKeyDown: (event: KeyboardEvent) => void;
  /** Runs before the editor pastes the text: a paste it prevents inserts nothing. */
  onPaste: (event: ClipboardEvent) => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const props = useRef({ value, onChange, onKeyDown, onPaste });
  props.current = { value, onChange, onKeyDown, onPaste };
  const options = useRef(new Compartment());
  const deferredKeys = useRef(false);
  const handleKey = (event: KeyboardEvent) => {
    // WebKit may finish composition before its confirming Enter (keyCode 229).
    if (event.defaultPrevented || event.isComposing || event.keyCode === 229) return;
    props.current.onKeyDown(event);
  };
  useImperativeHandle(ref, () => ({ focus: () => view.current?.focus() }), []);

  useEffect(() => {
    if (!host.current) return;
    const nav = host.current.ownerDocument.defaultView?.navigator;
    // Mobile CodeMirror defers native keys and replays them after reading the edited text.
    deferredKeys.current =
      /Android|Mobile/.test(nav?.userAgent ?? "") ||
      (/Apple Computer/.test(nav?.vendor ?? "") && (nav?.maxTouchPoints ?? 0) > 2);
    const { value: doc } = props.current;
    const created = new EditorView({
      parent: host.current,
      state: EditorState.create({
        doc,
        selection: { anchor: doc.length },
        extensions: [
          base,
          options.current.of(optionsFor(disabled, placeholder)),
          Prec.highest(
            EditorView.domEventHandlers({
              keydown: (event) => {
                if (deferredKeys.current) handleKey(event);
                return event.defaultPrevented;
              },
              paste: (event) => {
                props.current.onPaste(event);
                return event.defaultPrevented;
              },
              // Files dropped become attachments (the composer's drop target), never their text.
              drop: (event) => event.dataTransfer?.types.includes("Files") ?? false,
            }),
          ),
          EditorView.updateListener.of((update) => {
            if (editedByUser(update.transactions)) props.current.onChange(update.state.doc.toString());
          }),
        ],
      }),
    });
    view.current = created;
    return () => {
      created.destroy();
      view.current = null;
    };
    // The view is made once; its options and text follow below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    view.current?.dispatch({ effects: options.current.reconfigure(optionsFor(disabled, placeholder)) });
  }, [disabled, placeholder]);

  useEffect(() => {
    const current = view.current;
    const spec = current && externalText(current.state, value);
    if (spec) current.dispatch(spec);
  }, [value]);

  return (
    <div
      ref={host}
      className="min-w-0"
      onKeyDownCapture={(event) => {
        // Desktop shortcuts must run before CodeMirror's post-composition filter.
        if (!deferredKeys.current) handleKey(event.nativeEvent);
      }}
    />
  );
}

function optionsFor(disabled: boolean, placeholder: string): Extension {
  return [
    EditorState.readOnly.of(disabled),
    EditorView.editable.of(!disabled),
    placeholderWidget(placeholder),
  ];
}

/** The code previewed while the caret is elsewhere (lib/codePreview): chips and cards. */
const codePreviewPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = previewDecorations(view);
    }
    update(update: ViewUpdate) {
      if (
        update.docChanged ||
        update.selectionSet ||
        update.focusChanged ||
        update.viewportChanged ||
        syntaxTree(update.startState) !== syntaxTree(update.state)
      )
        this.decorations = previewDecorations(update.view);
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

/** A fenced block's language, in its card's corner while its fences are folded. */
class LanguageLabel extends WidgetType {
  constructor(readonly language: string) {
    super();
  }
  eq(other: LanguageLabel) {
    return other.language === this.language;
  }
  toDOM() {
    const label = document.createElement("span");
    label.className = "cm-composer-language";
    label.textContent = this.language;
    label.title = this.language;
    return label;
  }
}

// Code is not prose: the spelling is not checked there.
const noSpellcheck = { spellcheck: "false" };
const chip = Decoration.mark({ class: "cm-composer-chip", attributes: noSpellcheck });
const shownMark = Decoration.mark({ class: "cm-composer-mark" });
const hiddenMark = Decoration.replace({});

function previewDecorations(view: EditorView): DecorationSet {
  const { state } = view;
  const { from, to } = view.viewport;
  const selection = view.hasFocus ? state.selection.ranges : null;
  const { inline, blocks } = codePreview(state.doc, syntaxTree(state), selection, from, to);
  const decorations: Range<Decoration>[] = [];
  for (const { from, to, marks, open } of inline) {
    if (open) decorations.push(chip.range(from, to), ...marks.map((mark) => shownMark.range(mark.from, mark.to)));
    else decorations.push(chip.range(marks[0].to, marks[1].from), ...marks.map((mark) => hiddenMark.range(mark.from, mark.to)));
  }
  for (const { lines, fences, language, open } of blocks) {
    lines.forEach((at, index) => {
      const last = index === lines.length - 1;
      const fence = index === 0 || (last && fences.length === 2);
      const classes = ["cm-composer-code"];
      if (index === 0) classes.push("cm-composer-code-first");
      if (last) classes.push("cm-composer-code-last");
      if (fence && !open) classes.push(index === 0 && language ? "cm-composer-fence-label" : "cm-composer-fence-folded");
      decorations.push(Decoration.line({ class: classes.join(" "), attributes: noSpellcheck }).range(at));
    });
    fences.forEach((fence, index) => {
      const label = index === 0 && language ? new LanguageLabel(language) : undefined;
      decorations.push((open ? shownMark : Decoration.replace({ widget: label })).range(fence.from, fence.to));
    });
  }
  return Decoration.set(decorations, true);
}

// The Files editor's palette (CodeEditor) for the code; in the prose only the marks of markdown
// (fences, backticks, list marks) are dimmed, and inline code is set in the code font.
const highlightStyle = HighlightStyle.define([
  { tag: tags.monospace, fontFamily: "var(--font-mono)", fontSize: "var(--code-size)" },
  { tag: [tags.processingInstruction, tags.labelName], color: "var(--hl-meta)" },
  { tag: tags.comment, color: "var(--hl-comment)", fontStyle: "italic" },
  { tag: [tags.keyword, tags.modifier, tags.controlKeyword, tags.operatorKeyword, tags.definitionKeyword], color: "var(--hl-keyword)" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.attributeValue], color: "var(--hl-string)" },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.constant(tags.name)], color: "var(--hl-number)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.className], color: "var(--hl-function)" },
  { tag: [tags.typeName, tags.standard(tags.name), tags.namespace], color: "var(--hl-type)" },
  { tag: [tags.propertyName, tags.attributeName], color: "var(--hl-property)" },
  { tag: tags.tagName, color: "var(--hl-tag)" },
  { tag: tags.meta, color: "var(--hl-meta)" },
  { tag: tags.invalid, color: "#f06363" },
]);

// Sized and coloured as the textarea it replaced: it grows with the text up to 40% of the window,
// then scrolls.
const theme = EditorView.theme(
  {
    "&": { maxHeight: "40vh", minHeight: "38px", fontSize: "13px", color: "var(--color-fg)" },
    "&.cm-focused": { outline: "none" },
    ".cm-scroller": { overflow: "auto", fontFamily: "var(--font-sans)", lineHeight: "1.45" },
    ".cm-content": { padding: "10px 0 4px", caretColor: "var(--color-fg)" },
    ".cm-line": { padding: "0 12px" },
    ".cm-placeholder": { color: "var(--color-faint)" },
    // A fenced block's card: its lines side by side, the first and last ones closing it. Lines take
    // no vertical margin (the editor measures their boxes), and the fence lines, folded, give it
    // its top and bottom padding.
    ".cm-composer-code": {
      fontFamily: "var(--font-mono)",
      fontSize: "var(--code-size)",
      backgroundColor: "var(--code-bg)",
      margin: "0 8px",
      padding: "0 10px",
      borderLeft: "1px solid var(--color-line-strong)",
      borderRight: "1px solid var(--color-line-strong)",
    },
    ".cm-composer-code-first": {
      borderTop: "1px solid var(--color-line-strong)",
      borderTopLeftRadius: "6px",
      borderTopRightRadius: "6px",
    },
    ".cm-composer-code-last": {
      borderBottom: "1px solid var(--color-line-strong)",
      borderBottomLeftRadius: "6px",
      borderBottomRightRadius: "6px",
    },
    ".cm-composer-fence-folded": { lineHeight: "6px" },
    ".cm-composer-fence-label": { lineHeight: "18px", textAlign: "right" },
    // A very long info string ends with an ellipsis rather than widening the card.
    ".cm-composer-language": {
      display: "inline-block",
      maxWidth: "24ch",
      overflow: "hidden",
      textOverflow: "ellipsis",
      whiteSpace: "nowrap",
      verticalAlign: "top",
      fontFamily: "var(--font-sans)",
      fontSize: "10.5px",
      color: "var(--color-faint)",
      userSelect: "none",
    },
    ".cm-composer-chip": {
      fontFamily: "var(--font-mono)",
      fontSize: "var(--code-size)",
      backgroundColor: "var(--code-bg)",
      border: "1px solid var(--color-line-strong)",
      borderRadius: "4px",
      padding: "1px 4px",
      boxDecorationBreak: "clone",
      WebkitBoxDecorationBreak: "clone",
    },
    // Backticks and fences shown while the caret is on their code: there, but out of the way.
    ".cm-composer-mark": { opacity: "0.5" },
  },
  { dark: true },
);

const base: Extension = [
  history(),
  // Shift+Enter breaks the line as typed, without the indentation markdown would add.
  keymap.of([{ key: "Shift-Enter", run: insertNewline }, ...standardKeymap, ...historyKeymap]),
  markdown({ codeLanguages: fenceLanguage, addKeymap: false, pasteURLAsLink: false, completeHTMLTags: false }),
  syntaxHighlighting(highlightStyle),
  codePreviewPlugin,
  EditorView.lineWrapping,
  // The textarea's: spelling checked and corrected as in the other text fields. Tests and the
  // window's shortcuts find the composer by these, and whether it is empty (chatJump).
  EditorView.contentAttributes.of({ spellcheck: "true", autocorrect: "on", "aria-label": "Message", "data-testid": "composer" }),
  EditorView.contentAttributes.of((view) => ({ "data-composer": view.state.doc.length ? "text" : "empty" })),
  theme,
];
