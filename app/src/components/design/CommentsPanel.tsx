// Comments of a design chat (story 51), as the canvas shows them: a thread's popover at its pin
// (reply, resolve, send it alone to the agent, edit or delete one's own until sent), the bubble
// that writes a new one, and the Comments panel over the right of the canvas (by screen, open or
// resolved, "Send N new to the agent"). The rules live in lib/comments.ts; the calls stay in DesignCanvas.

import type { CommentAuthor, DesignComment, Mention } from "@chauffeur/protocol";
import { Check, CornerDownRight, MessageSquare, Pencil, RotateCcw, Send, Trash2, X } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { canChange, type Counts, placeOf, type Thread } from "@/lib/comments";
import { relativeTime } from "@/lib/format";
import { Badge, Button, cx, IconButton, Kbd } from "../ui";
import { type MentionSetup, MentionText, useMentionPicker } from "./Mentions";

function when(at: number): string {
  return relativeTime(new Date(at).toISOString());
}

/**
 * A text box for a comment: Enter submits, ⌘Enter submits and sends the new comments to the agent
 * (when `onSend` is given), ⇧Enter breaks a line, Esc cancels. On a shared design, `@` lists the
 * people to mention (`mentionSetup`); what it submits carries those the text still names.
 */
export function CommentComposer({
  initial = "",
  initialMentions,
  placeholder,
  submitLabel,
  busy,
  mentionSetup,
  onSubmit,
  onSend,
  onCancel,
}: {
  initial?: string;
  initialMentions?: readonly Mention[];
  placeholder: string;
  submitLabel: string;
  busy?: boolean;
  mentionSetup?: MentionSetup;
  onSubmit: (text: string, mentions: Mention[]) => void;
  /** Submits, then sends the new comments to the agent: none for a reviewer of a share. */
  onSend?: (text: string, mentions: Mention[]) => void;
  onCancel: () => void;
}) {
  const [text, setText] = useState(initial);
  const box = useRef<HTMLTextAreaElement>(null);
  const picker = useMentionPicker(mentionSetup, text, setText, box, initialMentions);
  useEffect(() => {
    box.current?.focus();
    box.current?.setSelectionRange(initial.length, initial.length);
  }, [initial.length]);
  const submit = (send: boolean) => {
    if (!text.trim() || busy) return;
    if (send && onSend) onSend(text, picker.mentions());
    else onSubmit(text, picker.mentions());
  };
  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (picker.onKeyDown(event)) return;
    if (event.key === "Escape") {
      // This box only: the thread or the bubble stays until the next Esc.
      event.preventDefault();
      event.stopPropagation();
      onCancel();
    } else if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      event.stopPropagation();
      submit(event.metaKey || event.ctrlKey);
    }
  };
  return (
    <div className="flex flex-col gap-1.5">
      <textarea
        ref={box}
        value={text}
        onChange={(event) => setText(event.target.value)}
        onKeyDown={onKeyDown}
        onSelect={picker.track}
        placeholder={mentionSetup ? `${placeholder} (@ to mention)` : placeholder}
        rows={2}
        className="w-full resize-none rounded-md border border-line-strong bg-panel px-2 py-1.5 text-[12.5px] text-fg outline-none placeholder:text-faint focus:border-accent/60"
        data-testid="comment-text"
      />
      {picker.list}
      <div className="flex items-center justify-end gap-1 whitespace-nowrap text-[11px] text-faint">
        <span className="mr-auto">
          <Kbd>⏎</Kbd> {submitLabel.toLowerCase()}
          {onSend && (
            <>
              {" "}
              · <Kbd>⌘⏎</Kbd> send
            </>
          )}{" "}
          · <Kbd>esc</Kbd> cancel
        </span>
        <Button size="xs" variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button size="xs" variant="primary" onClick={() => submit(false)} disabled={!text.trim()} busy={busy} data-testid="comment-submit">
          {submitLabel}
        </Button>
      </div>
    </div>
  );
}

/** One comment's line: author, when, state, text; edit or delete for the author until sent. */
function CommentLine({
  comment,
  me,
  reply,
  busy,
  mentionSetup,
  onEdit,
  onDelete,
}: {
  comment: DesignComment;
  me: CommentAuthor | null;
  reply?: boolean;
  busy: boolean;
  mentionSetup?: MentionSetup;
  onEdit: (id: string, text: string, mentions: Mention[]) => void;
  onDelete: (id: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const mine = me != null && canChange(comment, me);
  return (
    <div className={cx("group flex gap-1.5", reply && "pl-3")} data-testid={reply ? "comment-reply" : "comment-first"}>
      {reply && <CornerDownRight size={11} className="mt-[3px] shrink-0 text-faint" />}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5 text-[11px] leading-4">
          <span className="font-medium text-fg">{comment.author.name}</span>
          <span className="text-faint">{when(comment.at)}</span>
          {!comment.sent && (
            <Badge tone="accent" title="Not sent to the agent yet">
              new
            </Badge>
          )}
          {mine && !editing && (
            <span className="ml-auto hidden items-center gap-0.5 group-hover:inline-flex">
              <IconButton title="Edit" className="h-5 w-5" onClick={() => setEditing(true)} disabled={busy}>
                <Pencil size={11} />
              </IconButton>
              <IconButton title="Delete" className="h-5 w-5" onClick={() => onDelete(comment.id)} disabled={busy} data-testid="comment-delete">
                <Trash2 size={11} />
              </IconButton>
            </span>
          )}
        </div>
        {editing ? (
          <CommentComposer
            initial={comment.text}
            initialMentions={comment.mentions}
            placeholder="Your comment"
            submitLabel="Save"
            busy={busy}
            mentionSetup={mentionSetup}
            onSubmit={(text, mentions) => {
              onEdit(comment.id, text, mentions);
              setEditing(false);
            }}
            onCancel={() => setEditing(false)}
          />
        ) : (
          <div className="whitespace-pre-wrap text-[12.5px] leading-[18px] text-fg selectable">
            <MentionText text={comment.text} mentions={comment.mentions} me={me?.email} />
          </div>
        )}
      </div>
    </div>
  );
}

/** A thread: at its pin on the canvas, or in the panel. */
export function ThreadView({
  thread,
  me,
  busy,
  stale,
  mentionSetup,
  onReply,
  onReplyAndSend,
  onSendThread,
  onResolve,
  onEdit,
  onDelete,
  onClose,
}: {
  thread: Thread;
  me: CommentAuthor | null;
  busy: boolean;
  stale?: boolean;
  /** Who a reply or an edit may mention: none on a design that is not shared. */
  mentionSetup?: MentionSetup;
  onReply: (threadId: string, text: string, mentions: Mention[]) => void;
  /** Replies, then sends the new comments to the agent: none for a reviewer of a share. */
  onReplyAndSend?: (threadId: string, text: string, mentions: Mention[]) => void;
  /** Sends this thread alone to the agent: none for a reviewer of a share. */
  onSendThread?: (threadId: string) => void;
  onResolve: (threadId: string, resolved: boolean) => void;
  onEdit: (id: string, text: string, mentions: Mention[]) => void;
  onDelete: (id: string) => void;
  onClose?: () => void;
}) {
  const [replying, setReplying] = useState(false);
  const resolved = thread.comment.resolved != null;
  return (
    <div className="flex flex-col gap-2" data-testid="comment-thread">
      <div className="flex items-center gap-1.5 text-[11px] text-muted">
        <span className="inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-accent px-1 text-[10px] font-semibold text-white">{thread.number}</span>
        <span className="truncate" title={thread.comment.pin?.element?.selector}>
          {placeOf(thread.comment)}
        </span>
        {stale && (
          <Badge tone="neutral" title={thread.comment.quote ? "This passage is no longer in the file" : "The element under this pin is no longer in the screen"}>
            {thread.comment.quote ? "outdated" : "stale"}
          </Badge>
        )}
        {resolved && <Badge tone="success">resolved</Badge>}
        <span className="ml-auto inline-flex items-center gap-0.5">
          {onSendThread && (
            <IconButton
              title={SEND_THREAD}
              className="h-5 w-5"
              onClick={() => onSendThread(thread.comment.id)}
              disabled={busy}
              data-testid="comment-send-thread"
            >
              <Send size={11} />
            </IconButton>
          )}
          <IconButton
            title={resolved ? "Reopen" : "Resolve"}
            className="h-5 w-5"
            onClick={() => onResolve(thread.comment.id, !resolved)}
            disabled={busy}
            data-testid="comment-resolve"
          >
            {resolved ? <RotateCcw size={11} /> : <Check size={12} />}
          </IconButton>
          {onClose && (
            <IconButton title="Close (Esc)" className="h-5 w-5" onClick={onClose}>
              <X size={12} />
            </IconButton>
          )}
        </span>
      </div>
      <CommentLine comment={thread.comment} me={me} busy={busy} mentionSetup={mentionSetup} onEdit={onEdit} onDelete={onDelete} />
      {thread.replies.map((reply) => (
        <CommentLine key={reply.id} comment={reply} me={me} reply busy={busy} mentionSetup={mentionSetup} onEdit={onEdit} onDelete={onDelete} />
      ))}
      {replying ? (
        <CommentComposer
          placeholder="Reply"
          submitLabel="Reply"
          busy={busy}
          mentionSetup={mentionSetup}
          onSubmit={(text, mentions) => {
            onReply(thread.comment.id, text, mentions);
            setReplying(false);
          }}
          onSend={
            onReplyAndSend &&
            ((text, mentions) => {
              onReplyAndSend(thread.comment.id, text, mentions);
              setReplying(false);
            })
          }
          onCancel={() => setReplying(false)}
        />
      ) : (
        <button type="button" className="self-start text-[11.5px] text-accent hover:underline" onClick={() => setReplying(true)} data-testid="comment-reply-open">
          Reply
        </button>
      )}
    </div>
  );
}

const SEND_THREAD = "Send this thread to the agent, asking it to address it (after its turn, when it works)";

export type Filter = "open" | "resolved";

export function CommentsPanel({
  threads,
  counts,
  stale,
  busy,
  error,
  filter,
  labelOf,
  onFilter,
  onSend,
  onSendThread,
  onOpen,
  onResolve,
  onClose,
  me,
  unread,
}: {
  /** The reader, whose mentions are highlighted. */
  me?: string | null;
  /** The threads holding unread mentions of the reader. */
  unread?: ReadonlySet<string>;
  threads: Thread[];
  counts: Counts;
  /** The stale threads, by id. */
  stale: ReadonlySet<string>;
  busy: boolean;
  error: string | null;
  filter: Filter;
  labelOf: (screen: string) => string;
  onFilter: (filter: Filter) => void;
  /** None for a reviewer of a share: nothing goes to the agent from there. */
  onSend?: () => void;
  /** Sends one thread alone to the agent: none for a reviewer of a share. */
  onSendThread?: (threadId: string) => void;
  onOpen: (thread: Thread) => void;
  onResolve: (threadId: string, resolved: boolean) => void;
  onClose: () => void;
}) {
  const shown = threads.filter((t) => (filter === "open" ? !t.comment.resolved : t.comment.resolved != null));
  const screens = [...new Set(shown.map((t) => t.comment.screen))];
  return (
    <div
      role="dialog"
      aria-label="Comments"
      data-popover
      className="absolute inset-y-0 right-0 z-20 flex w-[38%] min-w-[300px] max-w-[520px] flex-col border-l border-line-strong bg-bg shadow-2xl"
      data-testid="comments-panel"
    >
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-line px-3">
        <MessageSquare size={13} className="text-faint" />
        <span className="font-semibold text-fg">Comments</span>
        <span className="text-[11.5px] text-faint">
          {counts.open} open · {counts.resolved} resolved
        </span>
        <IconButton title="Close (Esc)" className="ml-auto" onClick={onClose} data-testid="comments-close">
          <X size={14} />
        </IconButton>
      </div>
      <div className="flex shrink-0 items-center gap-1 border-b border-line px-3 py-1.5">
        <Button size="xs" variant={filter === "open" ? "secondary" : "ghost"} onClick={() => onFilter("open")} data-testid="comments-open">
          Open
        </Button>
        <Button size="xs" variant={filter === "resolved" ? "secondary" : "ghost"} onClick={() => onFilter("resolved")} data-testid="comments-resolved">
          Resolved
        </Button>
        {onSend && (
        <Button
          size="sm"
          variant="primary"
          className="ml-auto"
          disabled={counts.unsent === 0}
          busy={busy}
          onClick={onSend}
          title={
            counts.unsent === 0
              ? "Every comment was sent"
              : "Send the new comments to the agent, asking it to address them (after its turn, when it works)"
          }
          data-testid="comments-send"
        >
          {!busy && <Send size={12} />} Send {counts.unsent} new to the agent
        </Button>
        )}
      </div>
      {error && <div className="shrink-0 border-b border-line bg-danger/10 px-3 py-1.5 text-[12px] text-danger selectable">{error}</div>}
      <div className="min-h-0 flex-1 overflow-y-auto">
        {shown.length === 0 ? (
          <div className="p-6 text-center text-[12.5px] text-faint" data-testid="comments-empty">
            {filter === "open" ? (
              <>
                No open comments. Press <Kbd>C</Kbd> and click a screen.
              </>
            ) : (
              "Nothing resolved yet."
            )}
          </div>
        ) : (
          screens.map((screen) => (
            <div key={screen} className="border-b border-line">
              <div className="sticky top-0 bg-bg px-3 py-1.5 text-[11.5px] font-medium text-muted" title={screen}>
                {labelOf(screen)}
              </div>
              {shown
                .filter((t) => t.comment.screen === screen)
                .map((thread) => (
                  <button
                    key={thread.comment.id}
                    type="button"
                    className="flex w-full items-start gap-2 px-3 py-2 text-left hover:bg-hover"
                    onClick={() => onOpen(thread)}
                    data-testid="comments-entry"
                  >
                    <span className="mt-[1px] inline-flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full bg-accent px-1 text-[10px] font-semibold text-white">
                      {thread.number}
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center gap-1.5 text-[11px] text-muted">
                        <span className="truncate">{placeOf(thread.comment)}</span>
                        {stale.has(thread.comment.id) && <Badge tone="neutral">stale</Badge>}
                        {!thread.comment.sent && <Badge tone="accent">new</Badge>}
                        {unread?.has(thread.comment.id) && (
                          <Badge tone="accent" title="You are mentioned here">
                            @you
                          </Badge>
                        )}
                        <span className="ml-auto shrink-0 text-faint">{when(thread.comment.at)}</span>
                      </span>
                      <span className="line-clamp-2 text-[12.5px] text-fg">
                        <span className="font-medium">{thread.comment.author.name}:</span>{" "}
                        <MentionText text={thread.comment.text} mentions={thread.comment.mentions} me={me} />
                      </span>
                      {thread.replies.length > 0 && (
                        <span className="text-[11px] text-faint">{thread.replies.length === 1 ? "1 reply" : `${thread.replies.length} replies`}</span>
                      )}
                    </span>
                    {onSendThread && (
                      <IconButton
                        title={SEND_THREAD}
                        className="h-5 w-5 shrink-0"
                        onClick={(event) => {
                          event.stopPropagation();
                          onSendThread(thread.comment.id);
                        }}
                        disabled={busy}
                        data-testid="comments-entry-send"
                      >
                        <Send size={11} />
                      </IconButton>
                    )}
                    <IconButton
                      title={thread.comment.resolved ? "Reopen" : "Resolve"}
                      className="h-5 w-5 shrink-0"
                      onClick={(event) => {
                        event.stopPropagation();
                        onResolve(thread.comment.id, thread.comment.resolved == null);
                      }}
                      disabled={busy}
                    >
                      {thread.comment.resolved ? <RotateCcw size={11} /> : <Check size={12} />}
                    </IconButton>
                  </button>
                ))}
            </div>
          ))
        )}
      </div>
    </div>
  );
}
