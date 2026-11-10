// The comments on a passage of an agent's message in a shared workspace (story 53), by its guests
// who can suggest and by its owner, under that message as threads: in the owner's app and on the
// guest page. Who may comment replies, with `@` mentions, and deletes their own; a thread that
// mentions the reader stands out until it shows on screen. Also what writes a new comment: the
// Comment button over a selection in one agent's message, and the box it opens.

import type { Mention, ShareComment } from "@chauffeur/protocol";
import { AtSign, CornerDownRight, MessageSquareQuote, Trash2 } from "lucide-react";
import { createContext, useContext, useEffect, useRef, useState } from "react";
import { relativeTime } from "@/lib/format";
import { COMMENT_MAX, quoteOf, type ShareThread } from "@/lib/shareComments";
import { CommentComposer } from "../design/CommentsPanel";
import { type MentionSetup, MentionText, useMentionPicker } from "../design/Mentions";
import { Button, cx } from "../ui";

export type ShareComments = {
  /** By the key of the agent's message they show under (`placeComments`). */
  byItem: ReadonlyMap<string, ShareThread[]>;
  /** The reader's e-mail: theirs may be deleted, their mentions stand out. */
  me: string | null;
  onDelete: ((comment: ShareComment) => void) | null;
  /** Who may comment replies: true once the reply was taken (the box closes). */
  onReply: ((thread: ShareComment, text: string, mentions: Mention[]) => Promise<boolean> | boolean) | null;
  /** Who a reply may mention. */
  mentionSetup?: MentionSetup;
  /** The threads holding mentions of the reader they have not seen: seen once shown. */
  unread?: ReadonlySet<string>;
  onSeen?: (thread: string) => void;
  /** A thread to bring into view (a mention opened), once. */
  target?: string | null;
  onTargetShown?: () => void;
};

export const ShareCommentsContext = createContext<ShareComments | null>(null);

export function ShareCommentCards({ itemKey }: { itemKey: string }) {
  const shared = useContext(ShareCommentsContext);
  const threads = shared?.byItem.get(itemKey);
  if (!shared || !threads?.length) return null;
  return (
    <div className="mt-1.5 flex flex-col gap-1.5" data-testid="share-comments">
      {threads.map((thread) => (
        <ThreadCard key={thread.comment.id} thread={thread} shared={shared} />
      ))}
    </div>
  );
}

function ThreadCard({ thread, shared }: { thread: ShareThread; shared: ShareComments }) {
  const card = useRef<HTMLDivElement>(null);
  const id = thread.comment.id;
  const unread = shared.unread?.has(id) ?? false;
  const [replying, setReplying] = useState(false);
  const [flash, setFlash] = useState(false);
  // An unread mention is seen once half of its thread shows.
  const onSeen = shared.onSeen;
  useEffect(() => {
    const element = card.current;
    if (!unread || !element || !onSeen || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries.some((entry) => entry.isIntersecting)) return;
        onSeen(id);
        observer.disconnect();
      },
      { threshold: 0.5 },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [unread, id, onSeen]);
  const targeted = shared.target === id;
  const onTargetShown = shared.onTargetShown;
  useEffect(() => {
    if (!targeted) return;
    card.current?.scrollIntoView({ block: "center" });
    setFlash(true);
    onTargetShown?.();
  }, [targeted, onTargetShown]);
  useEffect(() => {
    if (!flash) return;
    const timer = setTimeout(() => setFlash(false), 1400);
    return () => clearTimeout(timer);
  }, [flash]);
  const onReply = shared.onReply;
  return (
    <div
      ref={card}
      className={cx(
        "rounded-md border px-3 py-2 transition-shadow",
        unread ? "border-accent/50 bg-accent/[0.06]" : "border-attention/30 bg-attention/[0.06]",
        flash && "ring-2 ring-accent/60",
      )}
      data-testid="share-comment"
      data-thread={id}
    >
      <CommentLine comment={thread.comment} shared={shared} badge={unread} />
      {thread.replies.map((reply) => (
        <div key={reply.id} className="mt-1.5 flex gap-1.5 border-t border-line/60 pt-1.5" data-testid="share-comment-reply">
          <CornerDownRight size={11} className="mt-0.5 shrink-0 text-faint" />
          <div className="min-w-0 flex-1">
            <CommentLine comment={reply} shared={shared} />
          </div>
        </div>
      ))}
      {onReply &&
        (replying ? (
          <div className="mt-1.5 border-t border-line/60 pt-1.5">
            <CommentComposer
              placeholder="Reply"
              submitLabel="Reply"
              mentionSetup={shared.mentionSetup}
              onSubmit={(text, mentions) => {
                void Promise.resolve(onReply(thread.comment, text.trim(), mentions)).then((taken) => taken && setReplying(false));
              }}
              onCancel={() => setReplying(false)}
            />
          </div>
        ) : (
          <button type="button" className="mt-1 text-[11.5px] text-muted hover:text-fg" onClick={() => setReplying(true)} data-testid="share-comment-reply-open">
            Reply
          </button>
        ))}
    </div>
  );
}

function CommentLine({ comment, shared, badge }: { comment: ShareComment; shared: ShareComments; badge?: boolean }) {
  return (
    <>
      <div className="flex items-center gap-1.5 text-[11.5px] text-muted">
        {!comment.reply_to && <MessageSquareQuote size={12} className="shrink-0 text-attention" />}
        <span className="font-medium text-fg" title={comment.author.email}>
          {comment.author.name}
        </span>
        <span>{relativeTime(new Date(comment.at).toISOString())}</span>
        {badge && (
          <span className="inline-flex items-center gap-0.5 rounded bg-accent/15 px-1 text-[10.5px] font-medium text-accent" title="You are mentioned here">
            <AtSign size={10} />
            you
          </span>
        )}
        {shared.onDelete && comment.author.email === shared.me && (
          <Button
            size="xs"
            variant="ghost"
            className="ml-auto"
            onClick={() => shared.onDelete?.(comment)}
            title={comment.reply_to ? "Delete your reply" : "Delete your comment and its replies"}
            data-testid="share-comment-delete"
          >
            <Trash2 size={11} />
          </Button>
        )}
      </div>
      {!comment.reply_to && <div className="mt-1 line-clamp-3 border-l-2 border-attention/50 pl-2 text-[12px] italic text-muted">{comment.quote}</div>}
      <div className="mt-1 whitespace-pre-wrap break-words text-[12.5px] text-fg selectable">
        <MentionText text={comment.text} mentions={comment.mentions} me={shared.me} />
      </div>
    </>
  );
}

/** A passage of an agent's message selected, and where the selection ends on screen. */
export type Passage = { message: string; quote: string; top: number; left: number };

/** The selection, when it lies in one agent's message inside `view`. */
export function selectedPassage(view: HTMLElement | null): Passage | null {
  const selection = window.getSelection();
  if (!view || !selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  const holder = (node: Node) => (node instanceof Element ? node : node.parentElement)?.closest<HTMLElement>("[data-message-id]") ?? null;
  const start = holder(range.startContainer);
  const message = start?.dataset.messageId;
  if (!start || !message || start !== holder(range.endContainer) || !view.contains(start)) return null;
  const quote = quoteOf(selection.toString());
  if (!quote) return null;
  const box = range.getBoundingClientRect();
  return { message, quote, top: box.bottom, left: box.right };
}

/** The Comment button under a selection: opens the box for it. */
export function CommentSelectionButton({ passage, onComment }: { passage: Passage; onComment: (passage: Passage) => void }) {
  return (
    <Button
      size="sm"
      variant="primary"
      className="fixed z-50 shadow-lg"
      style={{ top: passage.top + 6, left: Math.max(8, Math.min(passage.left - 40, window.innerWidth - 120)) }}
      // Keeps the selection while the button is pressed.
      onMouseDown={(event) => event.preventDefault()}
      onClick={() => {
        onComment(passage);
        window.getSelection()?.removeAllRanges();
      }}
      data-testid="share-comment-selection"
    >
      <MessageSquareQuote size={12} /> Comment
    </Button>
  );
}

/** A comment on the passage selected: for the owner and the guests, never the agent; `@` mentions one of them. */
export function PassageCommentBox({
  passage,
  mentionSetup,
  readers,
  onComment,
  onClose,
}: {
  passage: Pick<Passage, "quote">;
  mentionSetup?: MentionSetup;
  /** Who reads it, as the placeholder says. */
  readers: string;
  /** True once the comment was taken (the box closes). */
  onComment: (text: string, mentions: Mention[]) => Promise<boolean> | boolean;
  onClose: () => void;
}) {
  const [text, setText] = useState("");
  const box = useRef<HTMLTextAreaElement>(null);
  const picker = useMentionPicker(mentionSetup, text, setText, box);
  const submit = () => {
    const value = text.trim();
    if (!value || value.length > COMMENT_MAX) return;
    void Promise.resolve(onComment(value, picker.mentions())).then((taken) => taken && onClose());
  };
  return (
    <div className="shrink-0 px-5 pb-3 pt-1">
      <div className="mx-auto max-w-[860px] rounded-lg border border-attention/50 bg-panel focus-within:border-attention" data-testid="share-comment-box">
        <div className="line-clamp-2 border-b border-line px-3 py-1.5 text-[12px] italic text-muted">“{passage.quote}”</div>
        <textarea
          ref={box}
          autoFocus
          value={text}
          onChange={(event) => setText(event.target.value)}
          onSelect={picker.track}
          onKeyDown={(event) => {
            if (picker.onKeyDown(event)) return;
            if (event.key === "Escape") onClose();
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              submit();
            }
          }}
          rows={3}
          placeholder={`Comment on this passage: ${readers} read it, the agent does not${mentionSetup ? " (@ to mention)" : ""}`}
          className="block w-full resize-none bg-transparent px-3 py-2 text-[13px] text-fg outline-none placeholder:text-faint"
          data-testid="share-comment-input"
        />
        {picker.list && <div className="px-2 pb-1">{picker.list}</div>}
        <div className="flex items-center gap-2 px-2 pb-2">
          <span className="text-[11px] text-faint">{text.length > COMMENT_MAX ? `At most ${COMMENT_MAX} characters` : "⌘↵ to comment · Esc to cancel"}</span>
          <Button size="sm" variant="ghost" className="ml-auto" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" variant="primary" disabled={!text.trim() || text.length > COMMENT_MAX} onClick={submit} data-testid="share-comment-send">
            <MessageSquareQuote size={12} /> Comment
          </Button>
        </div>
      </div>
    </div>
  );
}
