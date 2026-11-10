// Mentions in a comment's text box (docs/adr/mentions-in-share-comments.md): typing `@` lists the
// people the share is open to, under the box; arrows move, Enter or Tab picks, Esc closes. The
// app's canvas, the reviewer page, the workspace share's comments in the app and on the guest page
// use the same hook. The rules live in lib/mentions.ts.

import type { CommentAuthor, Mention } from "@chauffeur/protocol";
import { UserPlus } from "lucide-react";
import { type KeyboardEvent, type RefObject, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { insertMention, type MentionOption, mentionOptions, type MentionQuery, mentionQuery, mentionsIn, textParts } from "@/lib/mentions";
import { cx } from "../ui";

/** Who a text box may mention: none on a design that is not shared. */
export type MentionSetup = {
  people: readonly CommentAuthor[];
  /** The writer's e-mail: never offered. */
  me?: string | null;
  /** The share's team domain: a full address of it is offered too. */
  teamDomain?: string | null;
  /** The share's owner may invite an address no one has ("Invite and mention"). */
  canInvite?: boolean;
  /** Whom someone who may not invite asks to: "Dan doesn't have access. Ask Carol." */
  owner?: string | null;
};

export type MentionPicker = {
  /** True when the key was the list's (the caller does nothing more with it). */
  onKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => boolean;
  /** Reads the `@…` before the caret again: after a click or a caret move. */
  track: () => void;
  /** The list under the box, while it offers someone. */
  list: React.ReactNode;
  /** The people the text names now. */
  mentions: () => Mention[];
  /** Forgets who was picked: after the comment was posted. */
  reset: () => void;
};

export function useMentionPicker(
  setup: MentionSetup | undefined,
  text: string,
  setText: (text: string) => void,
  box: RefObject<HTMLTextAreaElement | null>,
  initial?: readonly Mention[],
): MentionPicker {
  const [picked, setPicked] = useState<Mention[]>(() => [...(initial ?? [])]);
  const [query, setQueryState] = useState<MentionQuery | null>(null);
  const shown = useRef<MentionQuery | null>(null);
  const setQuery = (next: MentionQuery | null) => {
    shown.current = next;
    setQueryState(next);
  };
  const [active, setActive] = useState(0);
  const options: MentionOption[] = setup && query ? mentionOptions(setup.people, query.query, setup) : [];
  const open = options.length > 0;

  const enabled = setup != null;
  const track = useCallback(() => {
    const element = box.current;
    const next = element && enabled ? mentionQuery(element.value, element.selectionStart ?? element.value.length) : null;
    const current = shown.current;
    if (current?.start === next?.start && current?.query === next?.query) return;
    shown.current = next;
    setQueryState(next);
    setActive(0);
  }, [box, enabled]);
  // Where the caret goes once a name was put in: set before the next key can land elsewhere.
  const caretAfter = useRef<number | null>(null);
  useLayoutEffect(() => {
    const caret = caretAfter.current;
    if (caret == null) return;
    caretAfter.current = null;
    box.current?.focus();
    box.current?.setSelectionRange(caret, caret);
  }, [text, box]);
  // The text changed: the query with it (the caret is where the change left it).
  useEffect(() => track(), [text, track]);

  const pick = (option: MentionOption) => {
    const element = box.current;
    if (!query || option.kind === "no_access") return;
    const caret = element?.selectionStart ?? text.length;
    const next = insertMention(text, query, caret, option.person);
    setText(next.text);
    setPicked((current) => [...current.filter((p) => p.email !== option.person.email), option.person]);
    setQuery(null);
    caretAfter.current = next.caret;
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>): boolean => {
    if (!open) return false;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      setActive((index) => (index + step + options.length) % options.length);
      return true;
    }
    if (event.key === "Enter" || event.key === "Tab") {
      event.preventDefault();
      event.stopPropagation();
      pick(options[Math.min(active, options.length - 1)]);
      return true;
    }
    if (event.key === "Escape") {
      // The list only: the comment stays until the next Esc.
      event.preventDefault();
      event.stopPropagation();
      setQuery(null);
      return true;
    }
    return false;
  };

  const list = open ? <MentionList options={options} active={Math.min(active, options.length - 1)} owner={setup?.owner} onPick={pick} onHover={setActive} /> : null;
  return {
    onKeyDown,
    track,
    list,
    mentions: () => mentionsIn(text, picked),
    reset: () => setPicked([]),
  };
}

function MentionList({
  options,
  active,
  owner,
  onPick,
  onHover,
}: {
  options: MentionOption[];
  active: number;
  owner?: string | null;
  onPick: (option: MentionOption) => void;
  onHover: (index: number) => void;
}) {
  return (
    <div role="listbox" aria-label="Mention someone" className="max-h-40 overflow-y-auto rounded-md border border-line-strong bg-raised py-0.5 text-[12px] shadow-lg" data-testid="mention-list">
      {options.map((option, index) => (
        <button
          key={`${option.kind}:${option.person.email}`}
          type="button"
          role="option"
          aria-selected={index === active}
          // The box keeps the focus: the caret is where the name goes.
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => onPick(option)}
          onMouseEnter={() => onHover(index)}
          className={cx("flex w-full items-center gap-2 px-2 py-1 text-left", index === active ? "bg-hover text-fg" : "text-muted")}
          data-testid="mention-option"
        >
          {option.kind === "no_access" ? (
            <span className="min-w-0 truncate text-faint" data-testid="mention-no-access">
              <span className="text-fg">{option.person.email}</span> doesn't have access. Ask {owner}.
            </span>
          ) : option.kind === "invite" ? (
            <>
              <UserPlus size={12} className="shrink-0 text-accent" />
              <span className="min-w-0 truncate">
                Invite <span className="text-fg">{option.person.email}</span> and mention
              </span>
            </>
          ) : (
            <>
              <span className="font-medium text-fg">{option.person.name}</span>
              <span className="min-w-0 truncate text-faint">{option.person.email}</span>
              {option.kind === "team" && <span className="ml-auto shrink-0 text-[10.5px] text-faint">team</span>}
            </>
          )}
        </button>
      ))}
    </div>
  );
}

/** A comment's text with its mentions highlighted; the reader's own stronger. */
export function MentionText({ text, mentions, me }: { text: string; mentions?: readonly Mention[]; me?: string | null }) {
  return (
    <>
      {textParts(text, mentions).map((part, index) =>
        part.mention ? (
          <span
            key={index}
            title={part.mention.email}
            className={cx("rounded px-0.5 font-medium", part.mention.email === me ? "bg-accent/25 text-accent" : "bg-accent/10 text-accent")}
            data-testid="mention"
          >
            {part.text}
          </span>
        ) : (
          <span key={index}>{part.text}</span>
        ),
      )}
    </>
  );
}
