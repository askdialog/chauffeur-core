// The file viewer and editor of the Files tab: CodeMirror, coloured like the diff view. Loaded on
// demand (see FilesPanel), so the rest of the app does not carry it.

import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { python } from "@codemirror/lang-python";
import { rust } from "@codemirror/lang-rust";
import { sql } from "@codemirror/lang-sql";
import { xml } from "@codemirror/lang-xml";
import { yaml } from "@codemirror/lang-yaml";
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  type LanguageSupport,
  StreamLanguage,
} from "@codemirror/language";
import { dockerFile } from "@codemirror/legacy-modes/mode/dockerfile";
import { shell } from "@codemirror/legacy-modes/mode/shell";
import { toml } from "@codemirror/legacy-modes/mode/toml";
import { highlightSelectionMatches, search, searchKeymap } from "@codemirror/search";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  rectangularSelection,
} from "@codemirror/view";
import { editorTheme } from "@/lib/editorTheme";
import { Vim, vim } from "@replit/codemirror-vim";
import { useEffect, useRef } from "react";
import { useApp } from "@/state/app";

export type GotoLine = { line: number; nonce: number };

// `:w` saves as ⌘S does; Vim's ex commands are global, so each view registers its own save.
const saves = new WeakMap<EditorView, () => void>();
Vim.defineEx("write", "w", (cm) => saves.get(cm.cm6)?.());

export default function CodeEditor({
  docKey,
  value,
  path,
  readOnly,
  gotoLine,
  onChange,
  onSave,
}: {
  /** A new key starts a new document (fresh undo history). */
  docKey: string;
  value: string;
  path: string;
  readOnly: boolean;
  gotoLine: GotoLine | null;
  onChange: (text: string) => void;
  onSave: () => void;
}) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const handlers = useRef({ onChange, onSave });
  handlers.current = { onChange, onSave };
  const readOnlyCompartment = useRef(new Compartment());
  const vimMode = useApp((s) => s.keyboard.vim_mode);
  const vimCompartment = useRef(new Compartment());

  // One view, given a new state per document.
  useEffect(() => {
    if (!host.current) return;
    const created = new EditorView({ parent: host.current });
    view.current = created;
    saves.set(created, () => handlers.current.onSave());
    return () => {
      created.destroy();
      view.current = null;
    };
  }, []);

  useEffect(() => {
    const current = view.current;
    if (!current) return;
    current.setState(
      EditorState.create({
        doc: value,
        extensions: [
          // First, so that Vim reads the keys before the other keymaps.
          vimCompartment.current.of(vimExtension(vimMode)),
          base,
          languageFor(path) ?? [],
          readOnlyCompartment.current.of(readOnlyExtension(readOnly)),
          keymap.of([
            {
              key: "Mod-s",
              preventDefault: true,
              run: () => {
                handlers.current.onSave();
                return true;
              },
            },
          ]),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) handlers.current.onChange(update.state.doc.toString());
          }),
        ],
      }),
    );
    // The document is replaced only for a new key or a value that is not the one shown.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [docKey]);

  // A value from outside (a reload, a revert) replaces the text.
  useEffect(() => {
    const current = view.current;
    if (!current || current.state.doc.toString() === value) return;
    current.dispatch({ changes: { from: 0, to: current.state.doc.length, insert: value } });
  }, [value]);

  useEffect(() => {
    view.current?.dispatch({ effects: readOnlyCompartment.current.reconfigure(readOnlyExtension(readOnly)) });
  }, [readOnly]);

  useEffect(() => {
    view.current?.dispatch({ effects: vimCompartment.current.reconfigure(vimExtension(vimMode)) });
  }, [vimMode]);

  useEffect(() => {
    const current = view.current;
    if (!current || !gotoLine) return;
    const doc = current.state.doc;
    const line = doc.line(Math.min(Math.max(gotoLine.line, 1), doc.lines));
    current.dispatch({
      selection: { anchor: line.from, head: line.to },
      effects: EditorView.scrollIntoView(line.from, { y: "center" }),
    });
    current.focus();
  }, [gotoLine, docKey]);

  return <div ref={host} className="h-full min-h-0" data-testid="code-editor" />;
}

function readOnlyExtension(readOnly: boolean): Extension {
  return [EditorState.readOnly.of(readOnly), EditorView.editable.of(!readOnly)];
}

/** Vim's keys, with its mode shown under the text. `data-vim` marks the editor for the app's Esc
 * handlers that listen before it (see `fromVimEditor`). */
function vimExtension(on: boolean): Extension {
  if (!on) return [];
  return [
    vim({ status: true }),
    EditorView.editorAttributes.of({ "data-vim": "" }),
    // Esc belongs to Vim even in normal mode, where it does nothing: it must not go on to interrupt
    // the agent or close a panel of the app.
    EditorView.domEventHandlers({
      keydown: (event) => {
        if (event.key === "Escape") event.stopPropagation();
        return false;
      },
    }),
  ];
}

function languageFor(path: string): LanguageSupport | Extension | null {
  const name = path.slice(path.lastIndexOf("/") + 1).toLowerCase();
  if (name === "dockerfile" || name.endsWith(".dockerfile")) return StreamLanguage.define(dockerFile);
  if (name.startsWith(".env") || name === ".zshrc" || name === ".bashrc") return StreamLanguage.define(shell);
  const ext = name.includes(".") ? name.slice(name.lastIndexOf(".") + 1) : "";
  switch (ext) {
    case "ts":
    case "mts":
    case "cts":
      return javascript({ typescript: true });
    case "tsx":
      return javascript({ typescript: true, jsx: true });
    case "js":
    case "mjs":
    case "cjs":
      return javascript();
    case "jsx":
      return javascript({ jsx: true });
    case "py":
    case "pyi":
      return python();
    case "json":
    case "jsonc":
      return json();
    case "md":
    case "mdx":
      return markdown();
    case "css":
    case "scss":
      return css();
    case "html":
    case "vue":
      return html();
    case "yml":
    case "yaml":
      return yaml();
    case "rs":
      return rust();
    case "sql":
      return sql();
    case "xml":
    case "svg":
      return xml();
    case "sh":
    case "bash":
    case "zsh":
      return StreamLanguage.define(shell);
    case "toml":
      return StreamLanguage.define(toml);
    default:
      return null;
  }
}


const base: Extension = [
  lineNumbers(),
  highlightActiveLineGutter(),
  highlightSpecialChars(),
  history(),
  foldGutter(),
  drawSelection(),
  EditorState.allowMultipleSelections.of(true),
  indentOnInput(),
  bracketMatching(),
  rectangularSelection(),
  highlightActiveLine(),
  highlightSelectionMatches(),
  search({ top: true }),
  keymap.of([...defaultKeymap, ...searchKeymap, ...historyKeymap, ...foldKeymap, indentWithTab]),
  editorTheme,
];
