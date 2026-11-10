// Find in a chat (⌘F): matches over the chat's own data, not the page, since the list is
// virtualised and only the rows in view are in the DOM. Searched: the user's messages and the main
// agent's text as drawn (its markdown without the syntax, lib/markdown.ts), so that the count is
// what the page highlights; thinking, tool calls, requests and the subagents' work are left out. Pure, tested
// in chatFind.test.ts; the find bar is components/chat/ChatFind.tsx.

import type { ChatRow } from "./chat";
import { markdownText } from "./markdown";

/** A match in a text, as offsets into the original text: `end` is exclusive. */
export type TextMatch = { start: number; end: number };

/** A match in the chat: its row in the list, and which match it is within the row's text. */
export type ChatMatch = { index: number; key: string; occurrence: number };

/**
 * The text compared, lower-cased and without accents ("Élève" reads "eleve"), each run of
 * whitespace one space (the page lays out line breaks and the gaps between blocks its own way),
 * with where each of its characters comes from in the original text, so that a match maps back to it.
 */
export function foldText(text: string): { folded: string; from: number[]; to: number[] } {
  let folded = "";
  const from: number[] = [];
  const to: number[] = [];
  let offset = 0;
  let space = false;
  for (const char of text) {
    const end = offset + char.length;
    const blank = /\s/.test(char);
    if (blank && space) {
      offset = end;
      continue;
    }
    space = blank;
    const plain = blank ? " " : char.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
    for (let i = 0; i < plain.length; i++) {
      from.push(offset);
      to.push(end);
    }
    folded += plain;
    offset = end;
  }
  return { folded, from, to };
}

/** Where `query` appears in `text`, ignoring case and accents, without overlaps; none for a blank query. */
export function findInText(text: string, query: string): TextMatch[] {
  if (!query.trim()) return [];
  const needle = foldText(query).folded;
  // A query of accents alone folds to nothing.
  if (!needle) return [];
  const { folded, from, to } = foldText(text);
  const matches: TextMatch[] = [];
  for (let at = folded.indexOf(needle); at !== -1; at = folded.indexOf(needle, at + needle.length)) {
    matches.push({ start: from[at], end: to[at + needle.length - 1] });
  }
  return matches;
}

/** The text a row offers to the search, or null: the user's messages and the main agent's text, as drawn. */
export function searchableText(row: ChatRow): string | null {
  if (row.kind === "user") return row.text;
  if (row.kind === "assistant" && row.depth === 0) return markdownText(row.text);
  return null;
}

/** Every match of `query` in the chat, from its top. */
export function findInChat(rows: readonly ChatRow[], query: string): ChatMatch[] {
  if (!query.trim()) return [];
  const matches: ChatMatch[] = [];
  rows.forEach((row, index) => {
    const text = searchableText(row);
    if (!text) return;
    findInText(text, query).forEach((_, occurrence) => matches.push({ index, key: row.key, occurrence }));
  });
  return matches;
}

/**
 * The match a new search starts on: the first one from the row at the top of the view down, else
 * the last one above it (the closest to where the reader is). -1 when nothing matches.
 */
export function firstMatchFrom(matches: readonly ChatMatch[], topIndex: number): number {
  if (matches.length === 0) return -1;
  const below = matches.findIndex((match) => match.index >= topIndex);
  return below === -1 ? matches.length - 1 : below;
}

/** The next (`step` 1) or previous (-1) match, wrapping around at both ends; -1 when there is none. */
export function stepMatch(current: number, count: number, step: 1 | -1): number {
  if (count === 0) return -1;
  if (current < 0 || current >= count) return step === 1 ? 0 : count - 1;
  return (current + step + count) % count;
}

/** "2 of 5", or "No results" for a query that matches nothing; empty before anything is typed. */
export function matchLabel(current: number, count: number, query: string): string {
  if (!query.trim()) return "";
  if (count === 0) return "No results";
  return `${current + 1} of ${count}`;
}

/** A chat on screen, as ⌘F sees it: whether the key was typed in it, and when the user last clicked or focused it (0: never). */
export type FindCandidate = { typedIn: boolean; touched: number };

/**
 * Which of the chats on screen (in the order they were shown) ⌘F finds in, as each view takes the
 * keys of the pane last focused (WorkspaceViews): the one the key was typed in, else the one last
 * clicked or focused, else the last one shown (a floating chat over another). -1 when none is.
 */
export function findOwner(chats: readonly FindCandidate[]): number {
  const typedIn = chats.findIndex((chat) => chat.typedIn);
  if (typedIn !== -1) return typedIn;
  let owner = chats.length - 1;
  chats.forEach((chat, i) => {
    if (chat.touched > (chats[owner]?.touched ?? 0)) owner = i;
  });
  return owner;
}
