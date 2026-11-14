// The agent's questions, answered in place of the composer, as in Claude Code's terminal: a tab
// per question, numbered options, a free answer, a review before sending, Esc to answer in the
// chat instead.

import type { RequestResponse } from "@chauffeur/protocol";
import { Check, ChevronLeft, ChevronRight } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { QuestionItem } from "@/lib/chat";
import {
  answerOf,
  type Choice,
  choose,
  complete,
  confirmOther,
  goToTab,
  hasPreviews,
  initialState,
  moveCursor,
  needsReview,
  previewMarkdown,
  previewOf,
  rowsOf,
  typeOther,
} from "@/lib/questions";
import { guessLanguage } from "@/lib/highlight";
import { Markdown } from "../Markdown";
import { cx, Spinner } from "../ui";

export const DECLINED =
  "The user declined to answer these questions and will reply in the chat instead.";

export function QuestionPrompt({
  item,
  respond,
  onEscape,
}: {
  item: QuestionItem;
  respond: (requestId: string, response: RequestResponse) => Promise<void>;
  /** Esc sets the question aside rather than declining it: a child's, answered from its parent. */
  onEscape?: () => void;
}) {
  const questions = item.questions;
  const [state, setState] = useState(() => initialState(questions));
  const [busy, setBusy] = useState(false);
  /** The option row under the pointer: its preview shows over the focused one's. */
  const [hovered, setHovered] = useState<number | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const other = useRef<HTMLInputElement>(null);
  const review = needsReview(questions);
  const onReview = state.tab >= questions.length;

  useEffect(() => {
    root.current?.focus();
  }, [item.requestId]);
  useEffect(() => {
    if (state.editing) other.current?.focus();
    else if (!busy) root.current?.focus();
  }, [state.editing, busy]);

  const apply = async ({ state: next, send, decline }: Choice) => {
    setState(next);
    if (!send && !decline) return;
    setBusy(true);
    try {
      if (decline) await respond(item.requestId, { type: "deny", message: DECLINED });
      else {
        const answers: Record<string, string> = {};
        questions.forEach((q, i) => {
          answers[q.question] = answerOf(next, i);
        });
        await respond(item.requestId, { type: "answer", answers });
      }
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (busy || event.metaKey || event.ctrlKey || event.altKey) return;
    // The keyboard leads again: the preview follows the focused option, not the pointer.
    setHovered(null);
    const key = event.key;
    if (state.editing) {
      if (key === "Enter") {
        event.preventDefault();
        void apply(confirmOther(questions, state));
      } else if (key === "Escape" || key === "ArrowUp" || key === "ArrowDown") {
        event.preventDefault();
        setState(moveCursor(questions, state, key === "ArrowUp" ? -1 : key === "ArrowDown" ? 1 : 0));
      }
      return;
    }
    const row = onReview ? state.review : state.cursor[state.tab];
    if (key === "ArrowUp" || key === "k") setState(moveCursor(questions, state, -1));
    else if (key === "ArrowDown" || key === "j") setState(moveCursor(questions, state, 1));
    else if (key === "Enter" || key === " ") void apply(choose(questions, state, row));
    else if (/^[1-9]$/.test(key)) void apply(choose(questions, state, Number(key) - 1));
    else if (review && (key === "ArrowRight" || (key === "Tab" && !event.shiftKey))) setState(goToTab(questions, state, state.tab + 1));
    else if (review && (key === "ArrowLeft" || (key === "Tab" && event.shiftKey))) setState(goToTab(questions, state, state.tab - 1));
    else if (key === "Escape") {
      if (onEscape) onEscape();
      else void apply({ state, send: false, decline: true });
    }
    else return;
    event.preventDefault();
  };

  return (
    <div className="mx-auto w-full max-w-[860px] px-5 pb-4 pt-1">
      <div
        ref={root}
        tabIndex={0}
        onKeyDown={onKeyDown}
        className="max-h-[55vh] overflow-y-auto rounded-lg border border-accent/50 bg-raised px-4 py-3 shadow-lg outline-none focus-visible:border-accent"
        data-testid="question-prompt"
      >
        {review && (
          <div className="mb-3 flex flex-wrap items-center gap-1.5 text-[12px]" role="tablist">
            <ChevronLeft size={13} className="text-faint" />
            {questions.map((q, i) => {
              const answered = answerOf(state, i).length > 0;
              return (
                <button
                  key={q.question}
                  role="tab"
                  aria-selected={state.tab === i}
                  onClick={() => setState(goToTab(questions, state, i))}
                  className={cx(
                    "flex items-center gap-1 rounded px-1.5 py-0.5",
                    state.tab === i ? "bg-accent/20 text-fg" : "text-muted hover:text-fg",
                  )}
                >
                  <Box checked={answered} round={false} />
                  {q.header || `Question ${i + 1}`}
                </button>
              );
            })}
            <button
              role="tab"
              aria-selected={onReview}
              onClick={() => setState(goToTab(questions, state, questions.length))}
              className={cx(
                "flex items-center gap-1 rounded px-1.5 py-0.5",
                onReview ? "bg-accent/20 text-fg" : "text-muted hover:text-fg",
              )}
            >
              <Check size={11} className={complete(questions, state) ? "text-success" : "text-faint"} /> Submit
            </button>
            <ChevronRight size={13} className="text-faint" />
          </div>
        )}

        {onReview ? (
          <>
            <div className="mb-2 font-medium text-fg">Review your answers</div>
            <div className="mb-3 space-y-1.5">
              {questions.map((q, i) => (
                <div key={q.question}>
                  <div className="text-muted">● {q.question}</div>
                  <div className={cx("pl-4", answerOf(state, i) ? "text-fg" : "text-attention")}>
                    → {answerOf(state, i) || "no answer yet"}
                  </div>
                </div>
              ))}
            </div>
            <div className="mb-1 text-muted">Ready to send your answers?</div>
            {["Send answers", "Answer in the chat instead"].map((label, row) => (
              <OptionRow
                key={label}
                number={row + 1}
                label={label}
                active={state.review === row}
                onClick={() => void apply(choose(questions, state, row))}
              />
            ))}
          </>
        ) : (
          (() => {
            const question = questions[state.tab];
            const previews = hasPreviews(question);
            const options = (
              <div onMouseLeave={() => setHovered(null)} className={previews ? "min-w-0 @2xl:w-[38%] @2xl:shrink-0" : undefined}>
                {rowsOf(question).map((row, index) => {
                  const active = state.cursor[state.tab] === index;
                  if (row.kind === "option") {
                    const picked = state.picked[state.tab].includes(row.label);
                    return (
                      <OptionRow
                        key={row.label}
                        number={index + 1}
                        label={row.label}
                        description={row.description}
                        active={active}
                        box={question.multi_select ? picked : undefined}
                        picked={!question.multi_select && picked}
                        onClick={() => void apply(choose(questions, state, index))}
                        onHover={previews ? () => setHovered(index) : undefined}
                      />
                    );
                  }
                  if (row.kind === "next") {
                    return (
                      <OptionRow
                        key="next"
                        number={index + 1}
                        label={state.tab === questions.length - 1 ? "Review" : "Next question"}
                        active={active}
                        onClick={() => void apply(choose(questions, state, index))}
                        onHover={previews ? () => setHovered(index) : undefined}
                      />
                    );
                  }
                  return (
                    <div
                      key="other"
                      onClick={() => void apply(choose(questions, state, index))}
                      onMouseEnter={previews ? () => setHovered(index) : undefined}
                      className={cx("flex cursor-pointer items-baseline gap-2 rounded px-1.5 py-1", active && "bg-hover")}
                    >
                      <Cursor active={active} />
                      <span className="w-4 shrink-0 text-right text-faint">{index + 1}.</span>
                      {state.editing && active ? (
                        <input
                          ref={other}
                          value={state.other[state.tab]}
                          onChange={(e) => setState(typeOther(state, e.target.value))}
                          placeholder="Type your answer, then Enter"
                          className="min-w-0 flex-1 bg-transparent text-fg placeholder:text-faint focus:outline-none"
                          data-testid="question-other"
                        />
                      ) : (
                        <span className={state.other[state.tab] ? "text-fg" : "text-muted"}>
                          {state.other[state.tab] || "Type something…"}
                        </span>
                      )}
                    </div>
                  );
                })}
              </div>
            );
            const shown = previewOf(question, hovered ?? state.cursor[state.tab]);
            return (
              <>
                <div className="mb-2 font-medium text-fg selectable">{question.question}</div>
                {previews ? (
                  // Beside the options, or below them in a narrow chat.
                  <div className="@container">
                    <div className="flex flex-col gap-3 @2xl:flex-row">
                      {options}
                      <div
                        className="max-h-[38vh] min-h-[72px] min-w-0 flex-1 overflow-auto rounded-md border border-line bg-panel px-3 py-2 font-mono text-[12px]"
                        data-testid="question-preview"
                      >
                        {shown ? (
                          <Markdown text={previewMarkdown(shown, guessLanguage)} />
                        ) : (
                          <div className="text-[11.5px] text-faint">No preview for this choice</div>
                        )}
                      </div>
                    </div>
                  </div>
                ) : (
                  options
                )}
              </>
            );
          })()
        )}

        <div className="mt-3 flex items-center gap-2 text-[11px] text-faint">
          {busy && <Spinner size={10} />}
          <span>
            Enter to select · ↑↓ to move{review ? " · ←→ between questions" : ""} · Esc {onEscape ? "to set it aside" : "to answer in the chat"}
          </span>
        </div>
      </div>
    </div>
  );
}

function OptionRow({
  number,
  label,
  description,
  active,
  box,
  picked = false,
  onClick,
  onHover,
}: {
  number: number;
  label: string;
  description?: string;
  active: boolean;
  /** A multi-select option: its checkbox state. */
  box?: boolean;
  picked?: boolean;
  onClick: () => void;
  /** The pointer is over it: its preview shows. */
  onHover?: () => void;
}) {
  return (
    <div
      onClick={onClick}
      onMouseEnter={onHover}
      className={cx("flex cursor-pointer items-baseline gap-2 rounded px-1.5 py-1", active && "bg-hover")}
      data-option={label}
    >
      <Cursor active={active} />
      <span className="w-4 shrink-0 text-right text-faint">{number}.</span>
      {box !== undefined && <Box checked={box} round={false} />}
      <span className="min-w-0">
        <span className={cx(active || picked ? "text-fg" : "text-muted", picked && "font-medium")}>{label}</span>
        {picked && <Check size={11} className="ml-1.5 inline text-success" />}
        {description && <span className="block text-[11.5px] leading-snug text-faint">{description}</span>}
      </span>
    </div>
  );
}

function Cursor({ active }: { active: boolean }) {
  return <span className={cx("w-3 shrink-0 text-accent", !active && "invisible")}>❯</span>;
}

function Box({ checked, round }: { checked: boolean; round: boolean }) {
  return (
    <span
      className={cx(
        "inline-flex h-3 w-3 shrink-0 items-center justify-center self-center border",
        round ? "rounded-full" : "rounded-[3px]",
        checked ? "border-accent bg-accent text-white" : "border-faint",
      )}
    >
      {checked && <Check size={8} strokeWidth={3} />}
    </span>
  );
}
