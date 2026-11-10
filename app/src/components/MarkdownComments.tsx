// A markdown file with its comments (story 57): the rendered file, its commented passages
// highlighted, "Comment" on a selection, and the Comments column (threads on their passages, open
// or resolved). The app's Files tab and a reviewer's page of a shared file both draw it; what a
// comment does (the daemon, the share's socket) comes from the props. Passages: lib/passages.ts.

import type { CommentAuthor, DesignComment, Mention } from "@chauffeur/protocol";
import { MessageSquare, MessageSquareQuote, X } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { counts, type Thread, threadsOf } from "@/lib/comments";
import { findPassage, passageRange, rangeText } from "@/lib/passages";
import { QUOTE_MAX, quoteOf } from "@/lib/shareComments";
import { CommentComposer, type Filter, ThreadView } from "./design/CommentsPanel";
import type { MentionSetup } from "./design/Mentions";
import { Markdown } from "./Markdown";
import { Badge, Button, cx, IconButton } from "./ui";

/** The CSS highlights painting the passages (index.css): every open thread's, and the one shown. */
const ALL = "file-comments";
const ACTIVE = "file-comment-active";

/** What the comments do; `send` (⌘⏎) also sends the new comments to an agent, where one may. */
export type CommentActions = {
  add: (quote: string, text: string, mentions: Mention[], send: boolean) => Promise<void>;
  reply: (threadId: string, text: string, mentions: Mention[], send: boolean) => Promise<void>;
  edit: (id: string, text: string, mentions: Mention[]) => void;
  remove: (id: string) => void;
  resolve: (id: string, resolved: boolean) => void;
  /** The reader opened a thread (its mentions are read). */
  opened?: (id: string) => void;
};

/** A passage the user selected, and where the selection ends on screen. */
type Selected = { quote: string; top: number; left: number };

/** The user's selection, when it lies in the rendered file: its passage as a comment keeps it. */
function selectedPassage(article: HTMLElement | null): Selected | null {
  const selection = window.getSelection();
  if (!article || !selection || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  if (!article.contains(range.startContainer) || !article.contains(range.endContainer)) return null;
  // The selection's own text reads best (blocks apart), but must hold the rendered file's characters only.
  const exact = rangeText(range);
  const shown = selection.toString();
  const raw = findPassage(exact, shown) ? shown : exact;
  if (!raw.trim() || raw.length > QUOTE_MAX) return null;
  const box = range.getBoundingClientRect();
  return { quote: quoteOf(raw), top: box.bottom, left: box.right };
}

export function MarkdownComments({
  text,
  comments,
  me,
  error,
  busy,
  panel,
  onPanel,
  actions,
  canSend,
  sendBar,
  mentionSetup,
  unread,
  focus,
}: {
  text: string;
  comments: DesignComment[];
  me: CommentAuthor | null;
  error?: string | null;
  busy: boolean;
  panel: boolean;
  onPanel: (open: boolean) => void;
  actions: CommentActions;
  /** ⌘⏎ sends to an agent: in the app, not on a reviewer's page. */
  canSend: boolean;
  /** Under the filters: where the new comments go, in the app. */
  sendBar?: ReactNode;
  mentionSetup?: MentionSetup;
  /** Threads holding unread mentions of the reader. */
  unread?: ReadonlySet<string>;
  /** A thread to show (a link to it, a mention), each time `at` changes. */
  focus?: { id: string; at: number } | null;
}) {
  const article = useRef<HTMLDivElement>(null);
  const column = useRef<HTMLDivElement>(null);
  const threads = useMemo(() => threadsOf(comments), [comments]);
  const [selected, setSelected] = useState<Selected | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("open");
  const [stale, setStale] = useState<ReadonlySet<string>>(() => new Set());
  const ranges = useRef(new Map<string, Range>());

  // The open threads' passages, painted once the file is drawn; a passage not found is outdated.
  useEffect(() => {
    const frame = requestAnimationFrame(() => {
      const root = article.current;
      if (!root) return;
      const found = new Map<string, Range>();
      const missing = new Set<string>();
      for (const thread of threads) {
        const quote = thread.comment.quote;
        if (!quote || thread.comment.resolved) continue;
        const range = passageRange(root, quote);
        if (range) found.set(thread.comment.id, range);
        else missing.add(thread.comment.id);
      }
      ranges.current = found;
      setStale((known) => (known.size === missing.size && [...missing].every((id) => known.has(id)) ? known : missing));
      const highlights = typeof CSS !== "undefined" && "highlights" in CSS ? CSS.highlights : null;
      if (!highlights) return;
      highlights.set(ALL, new Highlight(...found.values()));
      const shown = active ? found.get(active) : undefined;
      if (shown) highlights.set(ACTIVE, new Highlight(shown));
      else highlights.delete(ACTIVE);
    });
    return () => cancelAnimationFrame(frame);
  }, [threads, text, active]);
  useEffect(
    () => () => {
      if (typeof CSS !== "undefined" && "highlights" in CSS) {
        CSS.highlights.delete(ALL);
        CSS.highlights.delete(ACTIVE);
      }
    },
    [],
  );

  /** Shows a thread: its passage in the file, and its card in the column. */
  const show = (id: string) => {
    setActive(id);
    onPanel(true);
    actions.opened?.(id);
    ranges.current.get(id)?.startContainer.parentElement?.scrollIntoView({ block: "center", behavior: "smooth" });
    requestAnimationFrame(() => column.current?.querySelector(`[data-thread="${id}"]`)?.scrollIntoView({ block: "nearest", behavior: "smooth" }));
  };
  const focused = useRef<number | null>(null);
  useEffect(() => {
    if (!focus || focused.current === focus.at || !threads.some((t) => t.comment.id === focus.id)) return;
    focused.current = focus.at;
    requestAnimationFrame(() => show(focus.id));
  });

  // A click on a highlighted passage shows its thread.
  const onClick = (event: React.MouseEvent) => {
    if (!window.getSelection()?.isCollapsed) return;
    const caret = document.caretRangeFromPoint?.(event.clientX, event.clientY);
    if (!caret) return;
    for (const [id, range] of ranges.current) {
      if (range.isPointInRange(caret.startContainer, caret.startOffset)) {
        show(id);
        return;
      }
    }
  };

  const tally = counts(comments);
  const shown = threads.filter((t) => (filter === "open" ? !t.comment.resolved : t.comment.resolved != null));
  // The draft goes once the comment is kept; a failure (told by the caller) leaves it to retry.
  const add = (quote: string, text: string, mentions: Mention[], send: boolean) =>
    void actions.add(quote, text, mentions, send).then(
      () => setDraft(null),
      () => {},
    );

  return (
    <div className="flex h-full min-h-0">
      <div
        className="h-full min-w-0 flex-1 overflow-y-auto px-8 py-6"
        onScroll={() => setSelected(null)}
        onMouseDown={() => setSelected(null)}
        // Read once the browser has settled the selection.
        onMouseUp={() => requestAnimationFrame(() => setSelected(selectedPassage(article.current)))}
        onClick={onClick}
        data-testid="markdown-preview"
      >
        <div ref={article} className="mx-auto max-w-[760px]">
          <Markdown text={text} />
        </div>
      </div>
      {selected && (
        <Button
          size="sm"
          variant="primary"
          className="fixed z-50 shadow-lg"
          style={{ top: selected.top + 6, left: Math.max(8, Math.min(selected.left - 40, window.innerWidth - 120)) }}
          // Keeps the selection while the button is pressed.
          onMouseDown={(event) => event.preventDefault()}
          onClick={() => {
            setDraft(selected.quote);
            setSelected(null);
            setFilter("open");
            onPanel(true);
            window.getSelection()?.removeAllRanges();
          }}
          data-testid="file-comment-selection"
        >
          <MessageSquareQuote size={12} /> Comment
        </Button>
      )}
      {panel && (
        <aside className="flex w-[340px] max-w-[85vw] shrink-0 flex-col border-l border-line bg-bg" aria-label="Comments" data-testid="file-comments">
          <div className="flex h-9 shrink-0 items-center gap-2 border-b border-line px-3">
            <MessageSquare size={13} className="text-faint" />
            <span className="font-semibold text-fg">Comments</span>
            <span className="text-[11.5px] text-faint">
              {tally.open} open · {tally.resolved} resolved
            </span>
            <IconButton title="Close" className="ml-auto" onClick={() => onPanel(false)}>
              <X size={14} />
            </IconButton>
          </div>
          <div className="flex shrink-0 items-center gap-1 border-b border-line px-3 py-1.5">
            <Button size="xs" variant={filter === "open" ? "secondary" : "ghost"} onClick={() => setFilter("open")}>
              Open
            </Button>
            <Button size="xs" variant={filter === "resolved" ? "secondary" : "ghost"} onClick={() => setFilter("resolved")}>
              Resolved
            </Button>
          </div>
          {sendBar}
          {error && <div className="shrink-0 border-b border-line bg-danger/10 px-3 py-1.5 text-[12px] text-danger selectable">{error}</div>}
          <div ref={column} className="min-h-0 flex-1 overflow-y-auto">
            {draft != null && (
              <div className="flex flex-col gap-1.5 border-b border-line px-3 py-2.5" data-testid="file-comment-draft">
                <div className="line-clamp-3 border-l-2 border-attention pl-2 text-[12px] italic text-muted">“{draft}”</div>
                <CommentComposer
                  placeholder="Comment on this passage"
                  submitLabel="Comment"
                  busy={busy}
                  mentionSetup={mentionSetup}
                  onSubmit={(text, mentions) => add(draft, text, mentions, false)}
                  onSend={canSend ? (text, mentions) => add(draft, text, mentions, true) : undefined}
                  onCancel={() => setDraft(null)}
                />
              </div>
            )}
            {shown.length === 0 && draft == null ? (
              <div className="p-6 text-center text-[12.5px] text-faint">
                {filter === "open" ? "No open comments. Select a passage of the file, then Comment." : "Nothing resolved yet."}
              </div>
            ) : (
              shown.map((thread: Thread) => (
                <div
                  key={thread.comment.id}
                  data-thread={thread.comment.id}
                  className={cx("border-b border-line px-3 py-2.5", active === thread.comment.id && "bg-hover")}
                  onClick={() => show(thread.comment.id)}
                >
                  {unread?.has(thread.comment.id) && (
                    <Badge tone="accent" title="You are mentioned here">
                      @you
                    </Badge>
                  )}
                  <ThreadView
                    thread={thread}
                    me={me}
                    busy={busy}
                    stale={stale.has(thread.comment.id)}
                    mentionSetup={mentionSetup}
                    onReply={(id, text, mentions) => void actions.reply(id, text, mentions, false).catch(() => {})}
                    onReplyAndSend={canSend ? (id, text, mentions) => void actions.reply(id, text, mentions, true).catch(() => {}) : undefined}
                    onResolve={actions.resolve}
                    onEdit={actions.edit}
                    onDelete={actions.remove}
                  />
                </div>
              ))
            )}
          </div>
        </aside>
      )}
    </div>
  );
}
