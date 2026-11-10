// The languages a fenced block of the chat composer is highlighted in, picked by its info string
// (```ts). The same ones the Files editor bundles: JavaScript, CSS and HTML come with the markdown
// mode anyway; the others load on the first block that names them, in the Files editor's chunks.

import { css } from "@codemirror/lang-css";
import { html } from "@codemirror/lang-html";
import { javascript } from "@codemirror/lang-javascript";
import { markdown } from "@codemirror/lang-markdown";
import { LanguageDescription, LanguageSupport, type StreamParser, StreamLanguage } from "@codemirror/language";

const language = (name: string, alias: string[], load: LanguageDescription["load"]) =>
  LanguageDescription.of({ name, alias, load });
const legacy = <T>(parser: StreamParser<T>) => new LanguageSupport(StreamLanguage.define(parser));

const languages: LanguageDescription[] = [
  language("TypeScript", ["ts", "mts", "cts"], async () => javascript({ typescript: true })),
  language("TSX", [], async () => javascript({ typescript: true, jsx: true })),
  language("JavaScript", ["js", "mjs", "cjs", "node"], async () => javascript()),
  language("JSX", [], async () => javascript({ jsx: true })),
  language("CSS", ["scss"], async () => css()),
  language("HTML", ["htm", "vue"], async () => html()),
  language("JSON", ["jsonc"], async () => (await import("@codemirror/lang-json")).json()),
  language("Python", ["py", "pyi"], async () => (await import("@codemirror/lang-python")).python()),
  language("Rust", ["rs"], async () => (await import("@codemirror/lang-rust")).rust()),
  language("SQL", [], async () => (await import("@codemirror/lang-sql")).sql()),
  language("XML", ["svg"], async () => (await import("@codemirror/lang-xml")).xml()),
  language("YAML", ["yml"], async () => (await import("@codemirror/lang-yaml")).yaml()),
  language("Markdown", ["md"], async () => markdown()),
  language("Shell", ["sh", "bash", "zsh", "console"], async () => legacy((await import("@codemirror/legacy-modes/mode/shell")).shell)),
  language("TOML", [], async () => legacy((await import("@codemirror/legacy-modes/mode/toml")).toml)),
  language("Dockerfile", ["docker"], async () => legacy((await import("@codemirror/legacy-modes/mode/dockerfile")).dockerFile)),
];

/** The language a fence's info string names (`ts`, `TypeScript`, `py title="x"`), or null. */
export function fenceLanguage(info: string): LanguageDescription | null {
  const name = /\S*/.exec(info)?.[0] ?? "";
  return name ? LanguageDescription.matchLanguageName(languages, name, true) : null;
}
