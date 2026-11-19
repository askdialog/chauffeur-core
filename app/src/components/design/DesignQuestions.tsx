import type { DesignQuestion, DesignQuestionOption, DesignQuestionResult, DesignVoteParams } from "@chauffeur/protocol";
import { Check, ChevronLeft, ChevronRight, Crosshair, MessageSquarePlus, Plus, SquarePen, X } from "lucide-react";
import { type ReactNode, type Ref, useEffect, useRef, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { Button, cx, IconButton, inputClass, Spinner } from "../ui";

export type QuestionAuthor = {
  options: DesignQuestionOption[];
  attaching: boolean;
  setAttaching: (active: boolean) => void;
  clear: () => void;
  remove: (file: string) => void;
  publish: ((question: DesignQuestion) => Promise<DesignQuestionResult[]>) | null;
  share: () => void;
};

export function QuestionsButton({ open, onClick, buttonRef }: { open: boolean; onClick: () => void; buttonRef: Ref<HTMLButtonElement> }) {
  return (
    <Button ref={buttonRef} size="sm" variant={open ? "secondary" : "ghost"} aria-label="Questions" title="Questions" aria-pressed={open} onClick={onClick} data-testid="canvas-questions">
      <SquarePen size={12} /> <span className="hidden @[640px]:inline">Questions</span>
    </Button>
  );
}

export function DesignQuestions({
  open, load, vote, author, locate, available, onClose,
}: {
  open: boolean;
  load: (() => Promise<DesignQuestionResult[]>) | null;
  vote?: (id: string, answer: DesignVoteParams) => Promise<DesignQuestionResult[]>;
  author?: QuestionAuthor;
  locate: (file: string) => void;
  available: ReadonlySet<string>;
  onClose: () => void;
}) {
  const [loaded, setLoaded] = useState<{ source: typeof load; results: DesignQuestionResult[] } | null>(null);
  const [loadFailure, setLoadFailure] = useState<{ source: typeof load; reason: string } | null>(null);
  const results = loaded?.source === load ? loaded.results : null;
  const readError = loadFailure?.source === load ? loadFailure.reason : null;
  const [actionError, setActionError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [retry, setRetry] = useState(0);
  const [active, setActive] = useState("");
  const [answers, setAnswers] = useState<Record<string, DesignVoteParams>>({});
  const [text, setText] = useState("");
  const [creating, setCreating] = useState(false);
  const draftId = useRef("");
  const latest = useRef(0);
  const activeWrite = useRef(false);
  const title = useRef<HTMLHeadingElement>(null);
  const createField = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    title.current?.focus();
  }, [open]);
  useEffect(() => {
    if (open && creating) createField.current?.focus();
  }, [open, creating]);

  useEffect(() => {
    if (!open || !load) return;
    let stopped = false;
    let pending = false;
    const refresh = async () => {
      if (pending || activeWrite.current) return;
      pending = true;
      const current = ++latest.current;
      try {
        const next = await load();
        if (!stopped && current === latest.current) {
          setLoaded({ source: load, results: next });
          setLoadFailure(null);
        }
      } catch (error) {
        if (!stopped && current === latest.current) setLoadFailure({ source: load, reason: errorMessage(error) });
      } finally {
        pending = false;
      }
    };
    void refresh();
    const timer = window.setInterval(() => { if (!document.hidden) void refresh(); }, 5_000);
    window.addEventListener("focus", refresh);
    return () => {
      stopped = true;
      window.clearInterval(timer);
      window.removeEventListener("focus", refresh);
    };
  }, [open, load, retry]);

  if (!open) return null;
  const questions = load ? results ?? [] : [];
  const current = questions.find((result) => result.question.id === active) ?? questions[0];
  const answer = current ? answers[current.question.id] ?? { file: "", note: "" } : { file: "", note: "" };
  const setAnswer = (id: string, value: DesignVoteParams) => setAnswers((previous) => ({ ...previous, [id]: value }));
  const select = (id: string) => {
    setActive(id);
    setActionError(null);
  };
  const finish = () => {
    setCreating(false);
    author?.clear();
    setActionError(null);
    title.current?.focus();
  };
  const change = async (action: () => Promise<DesignQuestionResult[]>, done: () => void) => {
    if (activeWrite.current) return;
    activeWrite.current = true;
    latest.current++;
    setBusy(true);
    setActionError(null);
    try {
      setLoaded({ source: load, results: await action() });
      setLoadFailure(null);
      done();
    } catch (error) {
      setActionError(errorMessage(error));
    } finally {
      setBusy(false);
      activeWrite.current = false;
    }
  };
  const localAuthor = creating ? author : undefined;
  const publish = localAuthor?.publish;
  const next = current && questions[(questions.indexOf(current) + 1) % questions.length];
  return (
    <aside aria-label="Design questions" data-popover data-testid="questions-panel"
      className="relative z-10 flex w-[min(50%,clamp(260px,36%,410px))] shrink-0 flex-col border-l border-line-strong bg-bg text-[13px]"
      onKeyDown={(event) => {
        if (event.key !== "Escape" || event.defaultPrevented) return;
        event.preventDefault();
        event.stopPropagation();
        if (localAuthor?.attaching) localAuthor.setAttaching(false);
        else onClose();
      }}>
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-line px-3">
        <SquarePen size={13} className="text-muted" />
        <h2 ref={title} tabIndex={-1} className="font-semibold focus:outline-none">Questions</h2>
        {questions.length > 0 && <span className="rounded bg-raised px-1.5 text-[11px] text-muted">{questions.length}</span>}
        <IconButton title="Close questions (Esc)" className="ml-auto" onClick={onClose}><X size={14} /></IconButton>
      </div>
      {readError && (
        <div role="alert" className="flex shrink-0 flex-wrap items-center gap-2 border-b border-line px-3 py-2 text-danger">
          <span>Could not load questions: {readError}</span>
          <Button size="sm" onClick={() => setRetry((value) => value + 1)}>Retry</Button>
        </div>
      )}
      {!creating && (questions.length > 0 || author) && (
        <div className="flex max-h-[30%] shrink-0 flex-col gap-1 overflow-auto border-b border-line px-3 py-2">
          {questions.map((result) => (
            <Button key={result.question.id} size="sm" aria-pressed={current === result} disabled={busy}
              variant={current === result ? "secondary" : "ghost"} className="h-auto min-h-8 shrink-0 !justify-start py-1 text-left"
              onClick={() => select(result.question.id)} title={result.question.text}>
              {result.votes?.some((vote) => vote.mine) ? <Check size={13} className="shrink-0 text-success" /> : <SquarePen size={13} className="shrink-0" />}
              <span className="min-w-0 whitespace-normal break-words">{result.question.text}</span>
              {vote && <span className="ml-auto shrink-0 rounded bg-raised px-1.5 text-[11px] text-muted">{result.votes ? "Voted" : "To vote"}</span>}
            </Button>
          ))}
          {author && <Button size="sm" className="w-fit" disabled={busy} onClick={() => {
            draftId.current = crypto.randomUUID();
            setText("");
            setCreating(true);
            setActionError(null);
          }}><Plus size={13} /> New question</Button>}
        </div>
      )}
      <div className="min-h-0 flex-1 overflow-auto px-4 py-4">
        {localAuthor ? (
          <>
            <h3 className="mb-3 text-[15px] font-semibold">New question</h3>
            <label className="block font-medium">
              Question
              <input ref={createField} className={cx(inputClass, "mt-1 w-full")} value={text} maxLength={240}
                onChange={(event) => setText(event.target.value)} disabled={busy} />
            </label>
            <p className="mt-3 text-muted">Response type <span className="ml-1 rounded border border-line bg-raised px-2 py-0.5 text-fg">Single choice</span></p>
            <div className="my-4 border-t border-line" />
            <h4 className="font-semibold">Attached variants <span className="ml-1 rounded bg-raised px-1.5 text-muted">{localAuthor.options.length}</span></h4>
            <p className="my-1 text-muted">Attach 2 to 20 screens from the canvas.</p>
            <Button size="sm" className="my-2" onClick={() => localAuthor.setAttaching(!localAuthor.attaching)} disabled={busy}>
              {localAuthor.attaching ? <Check size={13} /> : <Crosshair size={13} />}
              {localAuthor.attaching ? "Finish selecting" : "Select on canvas"}
            </Button>
            <div className="space-y-1.5">
              {localAuthor.options.map((option) => (
                <div key={option.file} className="flex min-h-8 items-center gap-2 rounded-md border border-line bg-sidebar px-2">
                  <span className="min-w-0 flex-1 truncate" title={option.label}>{option.label}</span>
                  <Locate option={option} available={available} locate={locate} />
                  <IconButton title={`Remove ${option.label}`} disabled={busy} onClick={() => localAuthor.remove(option.file)}><X size={12} /></IconButton>
                </div>
              ))}
            </div>
            <p className="mt-4 border-t border-line pt-3 text-muted">A justification is optional. Results include voters’ names.</p>
            {!publish && <p className="mt-2 text-muted">Share the design to publish this question.</p>}
          </>
        ) : current ? (
          <>
            <h3 className="mb-2 text-[15px] font-semibold leading-snug selectable">{current.question.text}</h3>
            {current.votes ? (
              <Results votes={current.votes} options={current.question.options} available={available} locate={locate} />
            ) : (
              <>
                <p className="mb-3 text-muted">Choose a variant. Use the locate button to highlight it on the canvas.</p>
                <fieldset className="min-w-0 divide-y divide-line overflow-hidden rounded-md border border-line bg-sidebar" disabled={busy}>
                  <legend className="sr-only">Variants</legend>
                  {current.question.options.map((option) => (
                    <div key={option.file} className={cx("flex items-center gap-1 pr-1", answer.file === option.file && "bg-accent/10")}>
                      <label className="flex min-h-8 min-w-0 flex-1 cursor-pointer items-center gap-2 px-2 py-1">
                        <input type="radio" name={current.question.id} checked={answer.file === option.file}
                          className="shrink-0 accent-accent" onChange={() => {
                            setAnswer(current.question.id, { ...answer, file: option.file });
                            if (available.has(option.file)) locate(option.file);
                          }} />
                        <span className="min-w-0 break-words">{option.label}</span>
                      </label>
                      <Locate option={option} available={available} locate={locate} />
                    </div>
                  ))}
                </fieldset>
                <label className="mt-3 block font-medium">
                  Justification <span className="font-normal text-muted">· optional</span>
                  <textarea className={cx(inputClass, "mt-1 min-h-16 w-full py-2")} rows={2} maxLength={1000} value={answer.note}
                    onChange={(event) => setAnswer(current.question.id, { ...answer, note: event.target.value })}
                    placeholder="Why this variant?" disabled={busy} />
                </label>
              </>
            )}
          </>
        ) : load && !results && !readError ? (
          <div role="status" className="flex items-center gap-2 text-muted"><Spinner /> Loading questions…</div>
        ) : !readError && (
          <p className="flex items-start gap-2 text-muted"><MessageSquarePlus size={15} className="mt-0.5 shrink-0" /> No questions have been published on this design yet.</p>
        )}
      </div>
      {actionError && <div role="alert" className="border-t border-line px-3 py-2 text-danger">{actionError}</div>}
      {localAuthor ? (
        <Footer>
          <Button size="sm" onClick={finish} disabled={busy}>Cancel</Button>
          {publish ? <Button variant="primary" busy={busy} disabled={!text.trim() || localAuthor.options.length < 2 || localAuthor.attaching || localAuthor.options.some(({ file }) => !available.has(file))}
            onClick={() => void change(() => publish({ id: draftId.current, text, options: localAuthor.options }), () => {
              setActive(draftId.current);
              finish();
            })}>Publish question</Button> : <Button variant="primary" onClick={localAuthor.share}>Share design</Button>}
        </Footer>
      ) : current && !current.votes && vote ? (
        <Footer>
          <span className="min-w-0 flex-1 text-[12px] text-muted">Your name and justification will be visible to invited reviewers.</span>
          <Button variant="primary" busy={busy} disabled={!answer.file} onClick={() => void change(() => vote(current.question.id, answer), () => title.current?.focus())}>Submit vote</Button>
        </Footer>
      ) : current?.votes && (
        <Footer>
          <span className="text-[12px] text-muted">Named results</span>
          {next && next !== current && <Button size="sm" onClick={() => select(next.question.id)}>{next === questions[0] ? <ChevronLeft size={13} /> : <ChevronRight size={13} />} {next === questions[0] ? "First question" : "Next question"}</Button>}
        </Footer>
      )}
    </aside>
  );
}

function Footer({ children }: { children: ReactNode }) {
  return <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t border-line px-4 py-3">{children}</div>;
}

function Locate({ option, available, locate }: { option: DesignQuestionOption; available: ReadonlySet<string>; locate: (file: string) => void }) {
  return (
    <IconButton className="shrink-0" title={available.has(option.file) ? `Locate ${option.label}` : `${option.label} is unavailable on the canvas`}
      disabled={!available.has(option.file)} onClick={() => locate(option.file)}><Crosshair size={13} /></IconButton>
  );
}

function Results({ votes, options, available, locate }: {
  votes: NonNullable<DesignQuestionResult["votes"]>;
  options: DesignQuestionOption[];
  available: ReadonlySet<string>;
  locate: (file: string) => void;
}) {
  const mine = votes.find((vote) => vote.mine);
  const labels = new Map(options.map((option) => [option.file, option.label]));
  return (
    <>
      {mine && <p role="status" className="mb-3 flex items-center gap-2 rounded-md border border-success/25 bg-success/10 px-2 py-2 text-success"><Check size={14} /> Vote recorded for {labels.get(mine.file)}</p>}
      <div className="mb-1 flex items-center justify-between gap-2">
        <h4 className="font-semibold">Vote count</h4>
        <span className="text-muted">{votes.length} {votes.length === 1 ? "vote" : "votes"}</span>
      </div>
      <ul className="divide-y divide-line">
        {options.map((option) => {
          const count = votes.filter((vote) => vote.file === option.file).length;
          return (
            <li key={option.file} className="flex items-center gap-2 py-1">
              <div className="min-w-0 flex-1">
                <p className="break-words text-muted">{option.label}</p>
                <div aria-hidden className="mt-1 h-1 overflow-hidden rounded-full bg-line-strong">
                  <div className="h-full rounded-full bg-accent" style={{ width: `${votes.length ? count / votes.length * 100 : 0}%` }} />
                </div>
              </div>
              <span className="tabular-nums">{count}</span>
              <Locate option={option} available={available} locate={locate} />
            </li>
          );
        })}
      </ul>
      <h4 className="mb-2 mt-4 border-t border-line pt-3 font-semibold">Who voted for what</h4>
      {votes.length === 0 && <p className="text-muted">Nobody has voted yet.</p>}
      <ul className="space-y-2">
        {votes.map((vote, index) => (
          <li key={index} className="break-words rounded-md border border-line bg-sidebar px-3 py-2 selectable">
            <div className="flex flex-wrap items-baseline gap-x-2"><strong>{vote.name}</strong>{vote.mine && <span className="text-muted">You</span>}<span className="rounded bg-accent/10 px-1 text-accent">{labels.get(vote.file)}</span></div>
            {vote.note && <p className="mt-1 whitespace-pre-wrap text-muted">{vote.note}</p>}
          </li>
        ))}
      </ul>
    </>
  );
}
