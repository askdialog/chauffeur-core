// The agent's questions (Claude's AskUserQuestion, Codex's request_user_input) answered the way
// Claude Code's terminal does: one tab per question, numbered options, a free answer, and a review
// before sending when there is more than one thing to answer.

import type { Question } from "@chauffeur/protocol";

export type QuestionState = {
  /** The question shown; `questions.length` is the review tab. */
  tab: number;
  /** The highlighted row of each question. */
  cursor: number[];
  /** The options picked, by question. */
  picked: string[][];
  /** The free answer typed, by question. */
  other: string[];
  /** The free answer's field has the keyboard. */
  editing: boolean;
  /** The review tab's highlighted row: 0 sends, 1 declines. */
  review: number;
};

/** The rows of a question: its options, the free answer, then "Next" for a multi-select. */
export type Row = { kind: "option"; label: string; description: string } | { kind: "other" } | { kind: "next" };

/**
 * The preview of a question's row, in markdown, when its option has one (Claude's AskUserQuestion
 * may give each option a code snippet or a mockup to compare); null otherwise.
 */
export function previewOf(question: Question, row: number): string | null {
  const preview = question.options[row]?.preview;
  return preview?.trim() ? preview : null;
}

/**
 * A preview as the Markdown component shows it. Claude Code asks for "markdown in a monospace
 * box", and models often send bare code or an ASCII mockup: kept verbatim in a code block (in
 * `language` when it can be told), since markdown would drop its indentation. Previews with
 * fenced blocks, and prose without indented lines, are markdown as given.
 */
export function previewMarkdown(preview: string, language: (code: string) => string | null = () => null): string {
  if (/^\s*(```|~~~)/m.test(preview)) return preview;
  const verbatim = /^( {2,}|\t)\S/m.test(preview) || /[\u2500-\u257f]/.test(preview);
  if (!verbatim) return preview;
  const code = preview.replace(/^\n+|\s+$/g, "");
  const longest = Math.max(2, ...[...code.matchAll(/`+/g)].map((m) => m[0].length));
  const fence = "`".repeat(longest + 1);
  return `${fence}${language(code) ?? ""}\n${code}\n${fence}`;
}

/** A question whose options show previews: the prompt makes room for them beside the options. */
export function hasPreviews(question: Question): boolean {
  return question.options.some((_, row) => previewOf(question, row) !== null);
}

export function rowsOf(question: Question): Row[] {
  const rows: Row[] = question.options.map((o) => ({ kind: "option", label: o.label, description: o.description }));
  rows.push({ kind: "other" });
  if (question.multi_select) rows.push({ kind: "next" });
  return rows;
}

/** One question answered with one pick sends at once; anything more is reviewed first. */
export function needsReview(questions: Question[]): boolean {
  return questions.length > 1 || questions.some((q) => q.multi_select);
}

export function initialState(questions: Question[]): QuestionState {
  return {
    tab: 0,
    cursor: questions.map(() => 0),
    picked: questions.map(() => []),
    other: questions.map(() => ""),
    editing: false,
    review: 0,
  };
}

export function answerOf(state: QuestionState, index: number): string {
  const typed = state.other[index]?.trim();
  return [...(state.picked[index] ?? []), ...(typed ? [typed] : [])].join(", ");
}

/** What the agent receives: each question's text and its answer (picks joined by commas). */
export function answersOf(questions: Question[], state: QuestionState): Record<string, string> {
  const answers: Record<string, string> = {};
  questions.forEach((q, i) => {
    answers[q.question] = answerOf(state, i);
  });
  return answers;
}

export function complete(questions: Question[], state: QuestionState): boolean {
  return questions.every((_, i) => answerOf(state, i).length > 0);
}

export function moveCursor(questions: Question[], state: QuestionState, delta: number): QuestionState {
  const clamp = (value: number, count: number) => Math.min(Math.max(value, 0), count - 1);
  if (state.tab >= questions.length) return { ...state, review: clamp(state.review + delta, 2), editing: false };
  const cursor = [...state.cursor];
  cursor[state.tab] = clamp(cursor[state.tab] + delta, rowsOf(questions[state.tab]).length);
  return { ...state, cursor, editing: false };
}

export function goToTab(questions: Question[], state: QuestionState, tab: number): QuestionState {
  const last = needsReview(questions) ? questions.length : questions.length - 1;
  return { ...state, tab: Math.min(Math.max(tab, 0), last), editing: false };
}

/** After a question is answered: the next unanswered one, else the review. */
function advance(questions: Question[], state: QuestionState): QuestionState {
  for (let i = 1; i <= questions.length; i++) {
    const index = (state.tab + i) % questions.length;
    if (!answerOf(state, index)) return { ...state, tab: index, editing: false };
  }
  return { ...state, tab: questions.length, editing: false };
}

export type Choice = { state: QuestionState; send: boolean; decline?: boolean };

/** Picks the row `row` of the current tab (Enter, a digit or a click). */
export function choose(questions: Question[], state: QuestionState, row: number): Choice {
  if (state.tab >= questions.length) {
    if (row === 1) return { state, send: false, decline: true };
    if (complete(questions, state)) return { state, send: true };
    // Something is unanswered: go back to it.
    const missing = questions.findIndex((_, i) => !answerOf(state, i));
    return { state: { ...state, tab: missing, editing: false }, send: false };
  }
  const question = questions[state.tab];
  const target = rowsOf(question)[row];
  if (!target) return { state, send: false };
  const cursor = [...state.cursor];
  cursor[state.tab] = row;
  const at = { ...state, cursor };
  if (target.kind === "other") return { state: { ...at, editing: true }, send: false };
  if (target.kind === "next") return { state: advance(questions, at), send: false };
  const picked = [...at.picked];
  const other = [...at.other];
  if (question.multi_select) {
    const list = picked[state.tab];
    picked[state.tab] = list.includes(target.label) ? list.filter((l) => l !== target.label) : [...list, target.label];
    return { state: { ...at, picked }, send: false };
  }
  picked[state.tab] = [target.label];
  other[state.tab] = "";
  const next = { ...at, picked, other };
  if (!needsReview(questions)) return { state: next, send: true };
  return { state: advance(questions, next), send: false };
}

export function typeOther(state: QuestionState, text: string): QuestionState {
  const other = [...state.other];
  other[state.tab] = text;
  return { ...state, other };
}

/** Enter in the free answer: it stands alone for a single choice, beside the picks otherwise. */
export function confirmOther(questions: Question[], state: QuestionState): Choice {
  const question = questions[state.tab];
  if (!question || !state.other[state.tab]?.trim()) return { state, send: false };
  const picked = [...state.picked];
  if (!question.multi_select) picked[state.tab] = [];
  const next = { ...state, picked, editing: false };
  if (!needsReview(questions)) return { state: next, send: true };
  return { state: question.multi_select ? next : advance(questions, next), send: false };
}
