import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags } from "@lezer/highlight";

// The palette of the diff view (index.css, highlight.js).
const highlightStyle = HighlightStyle.define([
  // The same colours as highlight.js's (index.css), so the user's terminal theme applies here too.
  { tag: [tags.comment, tags.quote], color: "var(--hl-comment)", fontStyle: "italic" },
  { tag: [tags.keyword, tags.modifier, tags.controlKeyword, tags.operatorKeyword, tags.definitionKeyword], color: "var(--hl-keyword)" },
  { tag: [tags.string, tags.special(tags.string), tags.regexp, tags.attributeValue], color: "var(--hl-string)" },
  { tag: [tags.number, tags.bool, tags.null, tags.atom, tags.constant(tags.name)], color: "var(--hl-number)" },
  { tag: [tags.function(tags.variableName), tags.function(tags.propertyName), tags.className], color: "var(--hl-function)" },
  { tag: [tags.typeName, tags.standard(tags.name), tags.namespace], color: "var(--hl-type)" },
  { tag: [tags.propertyName, tags.attributeName], color: "var(--hl-property)" },
  { tag: [tags.tagName, tags.heading], color: "var(--hl-tag)" },
  { tag: tags.heading, fontWeight: "600" },
  { tag: [tags.meta, tags.processingInstruction], color: "var(--hl-meta)" },
  { tag: tags.link, color: "#7b83f6", textDecoration: "underline" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: tags.strong, fontWeight: "600" },
  { tag: tags.invalid, color: "#f06363" },
]);

const theme = EditorView.theme(
  {
    "&": { height: "100%", backgroundColor: "var(--code-bg)", color: "var(--code-fg)", fontSize: "var(--code-size)" },
    ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.5" },
    ".cm-content": { caretColor: "var(--code-fg)", padding: "6px 0" },
    ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--code-fg)" },
    "&.cm-focused": { outline: "none" },
    "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
      backgroundColor: "rgba(123, 131, 246, 0.28)",
    },
    ".cm-activeLine": { backgroundColor: "rgba(255, 255, 255, 0.035)" },
    ".cm-activeLineGutter": { backgroundColor: "rgba(255, 255, 255, 0.035)", color: "#a0a0a8" },
    ".cm-gutters": { backgroundColor: "var(--code-bg)", color: "rgba(108, 108, 117, 0.7)", border: "none" },
    ".cm-lineNumbers .cm-gutterElement": { padding: "0 10px 0 12px" },
    ".cm-foldGutter .cm-gutterElement": { color: "#6c6c75" },
    ".cm-selectionMatch": { backgroundColor: "rgba(123, 131, 246, 0.15)" },
    ".cm-searchMatch": { backgroundColor: "rgba(240, 181, 69, 0.25)", outline: "1px solid rgba(240, 181, 69, 0.5)" },
    ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "rgba(240, 181, 69, 0.5)" },
    ".cm-matchingBracket": { backgroundColor: "rgba(123, 131, 246, 0.25)", outline: "none" },
    ".cm-panels": { backgroundColor: "#1d1d21", color: "#e4e4e7", borderTop: "1px solid #26262b" },
    ".cm-panels.cm-panels-top": { borderBottom: "1px solid #26262b" },
    ".cm-search": { fontFamily: "var(--font-sans)", fontSize: "12px" },
    ".cm-search input, .cm-search button": { fontSize: "12px" },
    ".cm-textfield": {
      backgroundColor: "#0f0f11",
      border: "1px solid #34343a",
      borderRadius: "4px",
      color: "#e4e4e7",
    },
    ".cm-button": {
      backgroundImage: "none",
      backgroundColor: "#222227",
      border: "1px solid #34343a",
      borderRadius: "4px",
      color: "#e4e4e7",
    },
    ".cm-panel button[name=close]": { color: "#a0a0a8" },
    ".cm-vim-panel": { fontFamily: "var(--font-mono)", fontSize: "12px", lineHeight: "1.6" },
    ".cm-vim-panel input": { color: "inherit", fontFamily: "inherit", fontSize: "inherit" },
  },
  { dark: true },
);


export const editorTheme = [syntaxHighlighting(highlightStyle), theme];
