// highlight.js with the languages a Dialog codebase uses, registered once.

import hljs from "highlight.js/lib/core";
import bash from "highlight.js/lib/languages/bash";
import css from "highlight.js/lib/languages/css";
import diff from "highlight.js/lib/languages/diff";
import dockerfile from "highlight.js/lib/languages/dockerfile";
import go from "highlight.js/lib/languages/go";
import graphql from "highlight.js/lib/languages/graphql";
import ini from "highlight.js/lib/languages/ini";
import java from "highlight.js/lib/languages/java";
import javascript from "highlight.js/lib/languages/javascript";
import json from "highlight.js/lib/languages/json";
import kotlin from "highlight.js/lib/languages/kotlin";
import makefile from "highlight.js/lib/languages/makefile";
import markdown from "highlight.js/lib/languages/markdown";
import python from "highlight.js/lib/languages/python";
import ruby from "highlight.js/lib/languages/ruby";
import rust from "highlight.js/lib/languages/rust";
import scss from "highlight.js/lib/languages/scss";
import shell from "highlight.js/lib/languages/shell";
import sql from "highlight.js/lib/languages/sql";
import swift from "highlight.js/lib/languages/swift";
import typescript from "highlight.js/lib/languages/typescript";
import xml from "highlight.js/lib/languages/xml";
import yaml from "highlight.js/lib/languages/yaml";

const languages = {
  bash,
  css,
  diff,
  dockerfile,
  go,
  graphql,
  ini,
  java,
  javascript,
  json,
  kotlin,
  makefile,
  markdown,
  python,
  ruby,
  rust,
  scss,
  shell,
  sql,
  swift,
  typescript,
  xml,
  yaml,
};
for (const [name, language] of Object.entries(languages)) hljs.registerLanguage(name, language);
hljs.registerAliases(["ts", "tsx"], { languageName: "typescript" });
hljs.registerAliases(["js", "jsx"], { languageName: "javascript" });
hljs.registerAliases(["sh", "zsh", "console"], { languageName: "bash" });
hljs.registerAliases(["py"], { languageName: "python" });
hljs.registerAliases(["rs"], { languageName: "rust" });
hljs.registerAliases(["toml"], { languageName: "ini" });
hljs.registerAliases(["html", "vue", "svg"], { languageName: "xml" });
hljs.registerAliases(["yml"], { languageName: "yaml" });

export function escapeHtml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

// A chat's rows mount again as they scroll back into view: their code blocks are not highlighted
// twice. Least recently used first, in Map order.
const HIGHLIGHTED_MAX = 200;
const highlighted = new Map<string, string>();

/** HTML of `code` highlighted as `language`, or escaped text when the language is unknown. */
export function highlight(code: string, language: string | null | undefined): string {
  if (!language || !hljs.getLanguage(language)) return escapeHtml(code);
  const key = `${language}\0${code}`;
  const cached = highlighted.get(key);
  if (cached !== undefined) {
    highlighted.delete(key);
    highlighted.set(key, cached);
    return cached;
  }
  let html: string;
  try {
    html = hljs.highlight(code, { language, ignoreIllegals: true }).value;
  } catch {
    return escapeHtml(code);
  }
  highlighted.set(key, html);
  const oldest = highlighted.keys().next();
  if (highlighted.size > HIGHLIGHTED_MAX && !oldest.done) highlighted.delete(oldest.value);
  return html;
}

/** The language a snippet given without one is most likely in, when the guess is a clear one. */
export function guessLanguage(code: string): string | null {
  try {
    const guess = hljs.highlightAuto(code);
    return guess.language && guess.relevance >= 5 ? guess.language : null;
  } catch {
    return null;
  }
}

/**
 * Highlights a file's lines one by one while carrying the parser state across lines, so that
 * multi-line strings and comments stay coloured. Returns one HTML string per line.
 */
export function highlightLines(lines: string[], language: string | null): string[] {
  if (!language || !hljs.getLanguage(language)) return lines.map(escapeHtml);
  // Highlight the block once, then split the HTML on newlines, re-opening spans per line.
  let html: string;
  try {
    html = hljs.highlight(lines.join("\n"), { language, ignoreIllegals: true }).value;
  } catch {
    return lines.map(escapeHtml);
  }
  const out: string[] = [];
  const open: string[] = [];
  let current = "";
  const tag = /<span[^>]*>|<\/span>|\n/g;
  let last = 0;
  for (let match = tag.exec(html); match; match = tag.exec(html)) {
    current += html.slice(last, match.index);
    last = match.index + match[0].length;
    if (match[0] === "\n") {
      out.push(current + "</span>".repeat(open.length));
      current = open.join("");
    } else if (match[0] === "</span>") {
      open.pop();
      current += match[0];
    } else {
      open.push(match[0]);
      current += match[0];
    }
  }
  current += html.slice(last);
  out.push(current + "</span>".repeat(open.length));
  return out;
}
