// A design chat's canvas (story 47): its pages, sections and screens, live from the design folder.
// Screens are framed from the daemon's own origin for them, sandboxed without allow-same-origin,
// and not interactive here (an overlay takes the pointer): a click selects, a drag pans, pinch or
// ⌘-wheel zooms. Full screen (a section's button) and Present show them interactive. "Send to
// source chat" writes the screens into a coding chat's draft; a screen Claude published to
// claude.ai has a link to it beside its label. "Export PNG" has the daemon render the selected
// screens (or a section's) as one image, kept in the design folder and put on the clipboard.
// Comment mode (C) pins comments on the screens (story 51): a click on a screen asks the framed
// screen for the element under it and opens a bubble; the Comments panel lists them and sends
// the new ones to the agent. "Keep these" sends it the selection, asking to drop the rest.

import type { CanvasResult, ChatInfo, CommentAuthor, DesignComment, Mention } from "@chauffeur/protocol";
import { Check, ImageDown, MessageSquare, MessageSquarePlus, MessageSquareText, Play, RotateCw, Scan, Send, Share2 } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { canvasRevision, changedScreens, layoutCanvas, modifiedTimes, noScreens, pageSections, percent, presentOrder, type ShownSection, ZOOM_STEPS } from "@/lib/canvas";
import type { ChatView } from "@/lib/chat";
import { chatLabel } from "@/lib/chatTabs";
import { counts as commentCounts, type Draft, keepMessage, threadsOf } from "@/lib/comments";
import { type ExportRequest, exportedMessage, sectionExport, selectionExport } from "@/lib/designExport";
import { chatsToSendTo, handoffMessage, pageScreens, publishedScreens, screensToSend, sourceChatOf } from "@/lib/designHandoff";
import { picturesScreens } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { isShared, shareTitle } from "@/lib/share";
import { chatsOf, client, markMentionSeen, openThreadDone, sendNow, sendToChat, setCanvasPage, setCanvasSelection, setDesignShare, toast, useApp } from "@/state/app";
import { ContextMenu, type MenuItem } from "../ContextMenu";
import { Button, cx, Select, type SelectOption, Spinner } from "../ui";
import { FullScreenSection, PresentScreens } from "./CanvasOverlays";
import { CommentComposer, CommentsPanel, type Filter, ThreadView } from "./CommentsPanel";
import { CanvasViewport } from "./CanvasViewport";
import type { MentionSetup } from "./Mentions";
import { ShareDialog } from "./ShareDialog";
import type { ScreenCommenting } from "./ScreenComments";
import { useCanvasView } from "./useCanvasView";
import { useScreenSnapshots } from "./useScreenSnapshots";
import { usePins } from "./usePins";

const NO_SELECTION: string[] = [];
/** How long a screen that changed keeps its outline. */
const FLASH_MS = 1000;
/** How long after a write the canvas is read again (writes come in bursts). */
const REFRESH_DELAY = 300;

type CanvasState = { result: CanvasResult | null; loading: boolean; error: string | null };

/**
 * The canvas read from the daemon: when shown, again (debounced) when the agent writes one of its
 * files or ends a turn, and on reconnection (the screens' origin changes with the daemon). The
 * screens whose file changed are named in `flash` for a moment.
 */
function useCanvas(chat: ChatInfo, view: ChatView) {
  const [state, setState] = useState<CanvasState>({ result: null, loading: true, error: null });
  const [flash, setFlash] = useState<Record<string, number>>({});
  const times = useRef<Map<string, number> | null>(null);
  const latest = useRef(0);
  const status = useApp((s) => s.connection.status);

  const load = useCallback(async () => {
    const call = ++latest.current;
    setState((s) => ({ ...s, loading: true }));
    try {
      const result = await client.call("design.canvas", { chat: chat.id });
      if (call !== latest.current) return;
      const after = modifiedTimes(result);
      const changed = changedScreens(times.current, after);
      times.current = after;
      setState({ result, loading: false, error: null });
      if (changed.length > 0) {
        const stamp = Date.now();
        setFlash((current) => ({ ...current, ...Object.fromEntries(changed.map((file) => [file, stamp])) }));
        setTimeout(
          () =>
            setFlash((current) =>
              Object.fromEntries(Object.entries(current).filter(([, at]) => at !== stamp)),
            ),
          FLASH_MS,
        );
      }
    } catch (error) {
      if (call !== latest.current) return;
      setState((s) => ({ result: s.result, loading: false, error: errorMessage(error) }));
    }
  }, [chat.id]);

  useEffect(() => {
    if (status === "open") void load();
  }, [status, load]);

  const revision = useMemo(
    () => canvasRevision(view.items, view.turns, state.result?.dir ?? null),
    [view.items, view.turns, state.result?.dir],
  );
  const seen = useRef(revision);
  useEffect(() => {
    if (seen.current === revision) return;
    seen.current = revision;
    const timer = setTimeout(() => void load(), REFRESH_DELAY);
    return () => clearTimeout(timer);
  }, [revision, load]);

  return { ...state, flash, reload: load, revision };
}

type CommentsState = { comments: DesignComment[]; me: CommentAuthor | null; error: string | null };

/** The comments read from the daemon: when shown, again when the canvas may have changed (the agent resolves them in the same folder), and after each change of the user's. */
function useComments(chatId: string, revision: string) {
  const [state, setState] = useState<CommentsState>({ comments: [], me: null, error: null });
  const status = useApp((s) => s.connection.status);
  const load = useCallback(async () => {
    try {
      const result = await client.call("design.comments", { chat: chatId });
      setState({ comments: result.comments, me: result.author, error: result.error ? `comments.json is invalid: ${result.error}` : null });
    } catch (error) {
      setState((s) => ({ ...s, error: errorMessage(error) }));
    }
  }, [chatId]);
  useEffect(() => {
    if (status === "open") void load();
  }, [status, load]);
  const seen = useRef(revision);
  useEffect(() => {
    if (seen.current === revision) return;
    seen.current = revision;
    const timer = setTimeout(() => void load(), REFRESH_DELAY);
    return () => clearTimeout(timer);
  }, [revision, load]);
  return { ...state, reload: load };
}

export function DesignCanvas({ chat, view, headerStart }: { chat: ChatInfo; view: ChatView; headerStart?: ReactNode }) {
  const { result, loading, error, flash, reload, revision } = useCanvas(chat, view);
  // The share (story 51): read when the canvas shows, then kept by the daemon's events; a
  // reviewer's comment bumps `comments_rev`, so the comments are read again.
  const share = useApp((s) => s.designShares[chat.id]);
  const [sharing, setSharing] = useState(false);
  const status = useApp((s) => s.connection.status);
  useEffect(() => {
    if (status !== "open") return;
    client
      .call("design.share_state", { chat: chat.id })
      .then(setDesignShare)
      .catch((error: unknown) => console.warn("the share's state", errorMessage(error)));
  }, [status, chat.id]);
  const commentsState = useComments(chat.id, `${revision}:${share?.comments_rev ?? 0}`);
  const [page, setPage] = useState(0);
  const pages = result?.pages ?? [];
  const pageIndex = Math.min(page, Math.max(0, pages.length - 1));
  const sections = useMemo(() => (result ? pageSections(result, pageIndex) : []), [result, pageIndex]);
  const layout = useMemo(() => layoutCanvas(sections), [sections]);
  const steps = useMemo(() => presentOrder(sections), [sections]);
  const selection = useApp((s) => s.canvasSelection[chat.id]) ?? NO_SELECTION;
  const select = useCallback((files: string[]) => setCanvasSelection(chat.id, files), [chat.id]);
  // The header's Open › Browser opens one of the page's screens.
  useEffect(() => setCanvasPage(chat.id, pageScreens(sections)), [chat.id, sections]);
  const published = useMemo(() => publishedScreens(view.items, result?.dir ?? null), [view.items, result?.dir]);

  const { zoom, area, setArea, moveView, fit, zoomTo } = useCanvasView(layout, pageIndex);
  const snapshots = useScreenSnapshots(layout, zoom, area, picturesScreens);
  const [fullScreen, setFullScreen] = useState<number | null>(null);
  const [presenting, setPresenting] = useState<number | null>(null);
  const [exporting, setExporting] = useState(false);
  const exportPng = async (request: ExportRequest) => {
    setExporting(true);
    try {
      const result = await client.call("design.export", { chat: chat.id, files: request.files, name: request.name });
      toast(exportedMessage(result));
    } catch (error) {
      toast(`Could not export the PNG: ${errorMessage(error)}`, "error");
    } finally {
      setExporting(false);
    }
  };
  const exportable = selectionExport(sections, selection);

  // --- comments ---
  const threads = useMemo(() => threadsOf(commentsState.comments), [commentsState.comments]);
  const counts = useMemo(() => commentCounts(commentsState.comments), [commentsState.comments]);
  const [commentMode, setCommentMode] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [openThread, setOpenThread] = useState<string | null>(null);
  const [panel, setPanel] = useState(false);
  const [filter, setFilter] = useState<Filter>("open");
  const [busy, setBusy] = useState(false);
  const { onFrame, onOverlayFrame, pins, stale, elementUnder } = usePins(threads);
  const labelOf = useCallback(
    (file: string) => {
      for (const section of sections) for (const screen of section.screens) if (screen.file === file) return screen.label || file;
      return file;
    },
    [sections],
  );

  const act = async (what: string, call: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await call();
      await commentsState.reload();
    } catch (error) {
      toast(`Could not ${what}: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(false);
    }
  };
  const startComment = (file: string, x: number, y: number, whole: boolean, frame?: HTMLIFrameElement | null) => {
    setOpenThread(null);
    setDraft({ file, x, y, whole, element: null });
    if (!whole) {
      void elementUnder(file, x, y, frame).then((element) => setDraft((d) => (d && d.file === file && d.x === x && d.y === y ? { ...d, element } : d)));
    }
  };
  // Mentions (docs/adr/mentions-in-share-comments.md): on a live share only, among the people it is
  // open to; the owner may invite someone else in the same stroke.
  const identity = useApp((s) => s.identity);
  const live = share?.share && !share.share.stopped ? share.share : null;
  const mentionSetup = useMemo<MentionSetup | undefined>(
    () =>
      live
        ? {
            people: live.people.length > 0 ? live.people : [live.owner, ...live.reviewers.map((email) => ({ email, name: email }))],
            me: identity?.email ?? live.owner.email,
            teamDomain: live.team ? identity?.team_domain : null,
            canInvite: true,
          }
        : undefined,
    [live, identity?.email, identity?.team_domain],
  );
  /** Invites the mentioned addresses the share is not open to, before the comment names them. */
  const inviteMentioned = async (mentions: Mention[]) => {
    if (!live || !mentionSetup) return;
    const teamDomain = mentionSetup.teamDomain;
    const newcomers = mentions
      .map((m) => m.email)
      .filter((email) => email !== live.owner.email && !live.reviewers.includes(email) && !(teamDomain && email.endsWith(`@${teamDomain}`)));
    if (newcomers.length === 0) return;
    setDesignShare(await client.call("design.share_update", { chat: chat.id, reviewers: [...live.reviewers, ...newcomers], team: live.team }));
    toast(`Invited ${newcomers.join(", ")}`);
  };
  const withMentions = (mentions: Mention[]) => (mentionSetup && mentions.length > 0 ? mentions : undefined);
  const postComment = async (d: Draft, text: string, mentions: Mention[]) => {
    await inviteMentioned(mentions);
    await client.call("design.comment_add", {
      chat: chat.id,
      screen: d.file,
      pin: d.whole ? undefined : { x: d.x, y: d.y, element: d.element ?? undefined },
      text,
      mentions: withMentions(mentions),
    });
    setDraft(null);
  };
  const postReply = async (threadId: string, text: string, mentions: Mention[]) => {
    await inviteMentioned(mentions);
    await client.call("design.comment_add", { chat: chat.id, screen: "", text, reply_to: threadId, mentions: withMentions(mentions) });
  };
  /** Sends the new comments to the agent as one message, and marks them sent. */
  const sendNew = async () => {
    const result = await client.call("design.comments_send", { chat: chat.id });
    if (result.sent === 0) {
      toast("Every comment was sent already");
      return;
    }
    if (await sendNow(chat, result.message)) toast(`${result.sent === 1 ? "1 comment" : `${result.sent} comments`} sent to the agent`);
  };
  const addComment = (d: Draft, text: string, mentions: Mention[]) => act("add the comment", () => postComment(d, text, mentions));
  // ⌘⏎ in a bubble or a reply: the comment is written, then the new ones go to the agent.
  const addCommentAndSend = (d: Draft, text: string, mentions: Mention[]) =>
    act("add and send the comment", async () => {
      await postComment(d, text, mentions);
      await sendNew();
    });
  const reply = (threadId: string, text: string, mentions: Mention[]) => act("reply", () => postReply(threadId, text, mentions));
  const replyAndSend = (threadId: string, text: string, mentions: Mention[]) =>
    act("reply and send", async () => {
      await postReply(threadId, text, mentions);
      await sendNew();
    });
  const editComment = (id: string, text: string, mentions: Mention[]) =>
    act("edit the comment", async () => {
      await inviteMentioned(mentions);
      await client.call("design.comment_edit", { chat: chat.id, id, text, mentions: mentionSetup ? mentions : undefined });
    });
  const deleteComment = (id: string) =>
    act("delete the comment", async () => {
      await client.call("design.comment_delete", { chat: chat.id, id });
      setOpenThread((open) => (open === id ? null : open));
    });
  const resolve = (id: string, resolved: boolean) => act(resolved ? "resolve the comment" : "reopen the comment", () => client.call("design.comment_resolve", { chat: chat.id, id, resolved }));
  const sendComments = () => act("send the comments", sendNew);
  const sendThread = (threadId: string) =>
    act("send the comment", async () => {
      const result = await client.call("design.comments_send", { chat: chat.id, thread: threadId });
      if (await sendNow(chat, result.message)) toast("Comment sent to the agent");
    });
  const keep = keepMessage(chat.id, sections, selection);
  const keepThese = async () => {
    if (!keep) return;
    if (await sendNow(chat, keep)) toast("Sent to the agent");
  };
  const escape = () => {
    if (draft) setDraft(null);
    else if (openThread) setOpenThread(null);
    else if (panel) setPanel(false);
    else if (commentMode) setCommentMode(false);
    else select([]);
  };
  const shownThread = openThread ? (threads.find((t) => t.comment.id === openThread) ?? null) : null;
  // The user's unread mentions on this share: badged in the panel, read once their thread opens.
  const allMentions = useApp((s) => s.mentions);
  const unreadHere = useMemo(
    () => (live ? allMentions.filter((m) => !m.seen && m.place === "design" && m.share_id === live.id) : []),
    [allMentions, live],
  );
  const unreadThreadIds = useMemo(() => new Set(unreadHere.map((m) => m.thread)), [unreadHere]);
  useEffect(() => {
    if (!openThread) return;
    for (const m of unreadHere) if (m.thread === openThread) void markMentionSeen(m.id);
  }, [openThread, unreadHere]);
  // A mention clicked in the sidebar's list: its thread opens once its comments are read.
  const threadTarget = useApp((s) => s.threadTarget);
  useEffect(() => {
    if (threadTarget?.chat !== chat.id || !threads.some((t) => t.comment.id === threadTarget.thread)) return;
    setDraft(null);
    setOpenThread(threadTarget.thread);
    openThreadDone();
  }, [threadTarget, chat.id, threads]);
  // Esc closes what is open wherever the focus is (a bubble's text box takes its own Esc first).
  const escapeRef = useRef(escape);
  escapeRef.current = escape;
  const somethingOpen = draft != null || openThread != null || panel || commentMode;
  useEffect(() => {
    if (!somethingOpen) return;
    const listener = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented) return;
      event.preventDefault();
      escapeRef.current();
    };
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, [somethingOpen]);

  const popover =
    draft ? (
      <div className="w-[280px] rounded-lg border border-line-strong bg-bg p-2 shadow-2xl" data-testid="comment-bubble">
        <div className="mb-1 truncate text-[11px] text-muted">
          {draft.whole ? "Whole screen" : draft.element?.text ? `On “${draft.element.text}”` : `At ${Math.round(draft.x)}%, ${Math.round(draft.y)}%`}
        </div>
        <CommentComposer
          placeholder="Your comment"
          submitLabel="Comment"
          busy={busy}
          mentionSetup={mentionSetup}
          onSubmit={(text, mentions) => void addComment(draft, text, mentions)}
          onSend={(text, mentions) => void addCommentAndSend(draft, text, mentions)}
          onCancel={() => setDraft(null)}
        />
      </div>
    ) : shownThread ? (
      <div className="w-[320px] rounded-lg border border-line-strong bg-bg p-2.5 shadow-2xl" data-testid="comment-popover">
        <ThreadView
          thread={shownThread}
          me={commentsState.me}
          busy={busy}
          stale={stale.has(shownThread.comment.id)}
          mentionSetup={mentionSetup}
          onReply={reply}
          onReplyAndSend={(threadId, text, mentions) => void replyAndSend(threadId, text, mentions)}
          onSendThread={(threadId) => void sendThread(threadId)}
          onResolve={resolve}
          onEdit={editComment}
          onDelete={deleteComment}
          onClose={() => setOpenThread(null)}
        />
      </div>
    ) : null;
  // Full screen and Present comment as the canvas does; the bubble shows only in front.
  const overlay = fullScreen != null || presenting != null;
  const commenting: ScreenCommenting = {
    commentMode,
    onToggleCommentMode: () => {
      setCommentMode((on) => !on);
      setDraft(null);
    },
    pins,
    draft,
    openThread: shownThread,
    onFrame: onOverlayFrame,
    onCommentAt: startComment,
    onOpenThread: (id) => {
      setDraft(null);
      setOpenThread(id);
    },
    onEscape: () => {
      if (draft) setDraft(null);
      else if (openThread) setOpenThread(null);
      else setCommentMode(false);
    },
    popover,
  };

  const present = () => {
    if (steps.length === 0) return;
    const start = steps.findIndex((step) => selection.includes(step.screen.file));
    setPresenting(Math.max(0, start));
  };

  const zoomOptions: Array<SelectOption<string>> = [
    { value: "fit", label: "Fit", name: "Fit the page", description: "⇧1" },
    ...ZOOM_STEPS.map((step) => ({
      value: String(step),
      label: percent(step),
      description: step === 1 ? "⇧0" : undefined,
    })),
  ];
  const zoomValue = ZOOM_STEPS.find((step) => Math.abs(step - zoom.scale) < 1e-6);

  const empty = result != null && noScreens(result);

  return (
    <div className="flex h-full min-h-0 min-w-0 flex-col" data-testid="design-canvas">
      {/* Narrow, the buttons keep their icons and titles, not their labels. */}
      <div className="@container flex h-8 shrink-0 items-center gap-1 border-b border-line px-2">
        {headerStart}
        {pages.length > 1 && (
          <Select
            value={String(pageIndex)}
            options={pages.map((p, i) => ({ value: String(i), label: p.name || `Page ${i + 1}` }))}
            onChange={(value) => {
              setPage(Number(value));
              select([]);
            }}
            title="Canvas page"
            testId="canvas-page"
            width={220}
          />
        )}
        {selection.length > 0 && (
          <span className="shrink-0 whitespace-nowrap px-1 text-[11.5px] text-faint" data-testid="canvas-selection">
            {selection.length === 1 ? "1 screen selected" : `${selection.length} screens selected`}
          </span>
        )}
        <div className="ml-auto flex items-center gap-0.5">
          {loading && result && <Spinner size={11} className="mr-1 text-faint" />}
          <Select
            value={zoomValue != null ? String(zoomValue) : "custom"}
            options={zoomOptions}
            shown={{ label: percent(zoom.scale) }}
            onChange={(value) => (value === "fit" ? fit() : zoomTo(Number(value)))}
            title="Zoom"
            testId="canvas-zoom"
            width={170}
            disabled={layout.sections.length === 0}
          />
          <Button size="sm" variant="ghost" onClick={fit} disabled={layout.sections.length === 0} title="Fit the page (⇧1)" data-testid="canvas-fit">
            <Scan size={12} /> <span className="hidden @[960px]:inline">Fit</span>
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={present}
            disabled={steps.length === 0}
            title="Present: one screen at a time, interactive (← → between screens, Esc leaves)"
            data-testid="canvas-present"
          >
            <Play size={12} /> <span className="hidden @[960px]:inline">Present</span>
          </Button>
          <Button
            size="sm"
            variant={commentMode ? "secondary" : "ghost"}
            onClick={() => {
              setCommentMode((on) => !on);
              setDraft(null);
            }}
            disabled={layout.sections.length === 0}
            title={commentMode ? "Leave Comment mode (C or Esc)" : "Comment mode (C): click a screen to comment on it, its label for the whole screen"}
            aria-pressed={commentMode}
            data-testid="canvas-comment-mode"
          >
            <MessageSquarePlus size={12} /> <span className="hidden @[960px]:inline">Comment</span>
          </Button>
          <Button
            size="sm"
            variant={panel ? "secondary" : "ghost"}
            onClick={() => setPanel((on) => !on)}
            title="The comments, by screen, and sending the new ones to the agent"
            aria-pressed={panel}
            data-testid="canvas-comments"
          >
            <MessageSquareText size={12} /> <span className="hidden @[960px]:inline">Comments</span>
            {counts.open > 0 && <span className="rounded-full bg-accent/15 px-1 text-[10.5px] text-accent">{counts.open}</span>}
            {counts.unsent > 0 && <span className="h-1.5 w-1.5 rounded-full bg-accent" title={`${counts.unsent} not sent to the agent`} />}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void keepThese()}
            disabled={!keep}
            title={keep ? "Ask the agent to keep the selected screens and remove the other sections" : "Select the screens to keep (click one, ⇧-click to add)"}
            data-testid="canvas-keep"
          >
            <Check size={12} /> <span className="hidden @[960px]:inline">Keep these</span>
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => exportable && void exportPng(exportable)}
            disabled={!exportable || exporting}
            busy={exporting}
            title={
              exportable
                ? `One PNG of ${exportable.files.length === 1 ? "the selected screen" : `the ${exportable.files.length} selected screens`}, kept in the design folder's exports/ and put on the clipboard`
                : "Select screens to export (click one, ⇧-click to add)"
            }
            data-testid="canvas-export"
          >
            {!exporting && <ImageDown size={12} />} <span className="hidden @[960px]:inline">Export PNG</span>
          </Button>
          <Button
            size="sm"
            variant={isShared(share) ? "secondary" : "ghost"}
            onClick={() => setSharing(true)}
            title={shareTitle(share)}
            data-testid="canvas-share"
          >
            <Share2 size={12} /> <span className="hidden @[960px]:inline">Share</span>
            {isShared(share) && (
              <span
                className={cx("h-1.5 w-1.5 rounded-full", share?.connection.state === "online" ? "bg-success" : share?.connection.state === "failed" ? "bg-danger" : "bg-faint")}
                data-testid="canvas-share-dot"
              />
            )}
            {(share?.share?.viewers.length ?? 0) > 0 && <span className="rounded-full bg-accent/15 px-1 text-[10.5px] text-accent">{share?.share?.viewers.length}</span>}
          </Button>
          <SendToChat chat={chat} sections={sections} selection={selection} />
        </div>
      </div>
      {sharing && <ShareDialog target={{ kind: "design", chat: chat.id }} state={share} onClose={() => setSharing(false)} />}
      {result?.error && (
        <div className="flex shrink-0 items-center gap-2 border-b border-line bg-attention/10 px-3 py-1.5 text-[12px] text-attention" data-testid="canvas-invalid">
          <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-attention" />
          <span className="min-w-0 selectable">canvas.json is invalid: {result.error}. The screens are sorted automatically.</span>
        </div>
      )}
      {error && result && (
        <div className="flex shrink-0 items-center gap-2 border-b border-line bg-danger/10 px-3 py-1.5 text-[12px] text-danger">
          <span className="min-w-0 flex-1 truncate" title={error}>
            Could not read the canvas again: {error}
          </span>
          <button className="shrink-0 text-accent hover:underline" onClick={() => void reload()}>
            Retry
          </button>
        </div>
      )}
      <div className="relative min-h-0 flex-1">
        {!result ? (
          error ? (
            <div className="flex h-full flex-col items-center justify-center gap-2 p-6 text-center" data-testid="canvas-error">
              <div className="max-w-md text-[12.5px] text-danger selectable">Could not read the canvas: {error}</div>
              <Button size="sm" onClick={() => void reload()} busy={loading}>
                <RotateCw size={12} /> Retry
              </Button>
            </div>
          ) : (
            <div className="flex h-full items-center justify-center text-faint">
              <Spinner size={16} />
            </div>
          )
        ) : empty ? (
          <div className="flex h-full items-center justify-center p-6 text-center text-[12.5px] text-faint" data-testid="canvas-empty">
            No screens yet. Describe what you want to see in the chat.
          </div>
        ) : (
          <CanvasViewport
            key="viewport"
            layout={layout}
            zoom={zoom}
            area={area}
            setZoom={moveView}
            onArea={setArea}
            selection={selection}
            onSelect={select}
            flash={flash}
            commentMode={commentMode}
            pins={pins}
            draft={draft}
            openThread={shownThread}
            onFrame={onFrame}
            onCommentAt={startComment}
            onOpenThread={commenting.onOpenThread}
            onEscape={escape}
            onToggleCommentMode={commenting.onToggleCommentMode}
            popover={overlay ? null : popover}
            onFullScreen={setFullScreen}
            onExport={(index) => {
              const request = sections[index] && sectionExport(sections[index]);
              if (request && !exporting) void exportPng(request);
            }}
            onFit={fit}
            onActualSize={() => zoomTo(1)}
            published={published}
            snapshots={picturesScreens ? snapshots : undefined}
          />
        )}
      {panel && (
          <CommentsPanel
            me={identity?.email}
            unread={unreadThreadIds}
            threads={threads}
            counts={counts}
            stale={stale}
            busy={busy}
            error={commentsState.error}
            filter={filter}
            labelOf={labelOf}
            onFilter={setFilter}
            onSend={() => void sendComments()}
            onSendThread={(threadId) => void sendThread(threadId)}
            onOpen={(thread) => {
              setDraft(null);
              setOpenThread(thread.comment.id);
            }}
            onResolve={resolve}
            onClose={() => setPanel(false)}
          />
        )}
      </div>
      {fullScreen != null && sections[fullScreen] && (
        <FullScreenSection section={sections[fullScreen]} onClose={() => setFullScreen(null)} commenting={commenting} />
      )}
      {presenting != null && steps.length > 0 && (
        <PresentScreens steps={steps} start={presenting} published={published} onClose={() => setPresenting(null)} commenting={commenting} />
      )}
    </div>
  );
}

/**
 * "Send to source chat": the selected screens (else the page's) into the source chat's draft, then
 * that chat shows; nothing is sent. Without a source chat, "Send to a chat…" lists the
 * workspace's other chats, and a new one.
 */
function SendToChat({ chat, sections, selection }: { chat: ChatInfo; sections: ShownSection[]; selection: string[] }) {
  const chats = useApp((s) => s.chats);
  const state = useApp((s) => s.workspaces[chat.workspace_id]?.state);
  const all = useMemo(() => chatsOf(chats, chat.workspace_id), [chats, chat.workspace_id]);
  const source = sourceChatOf(chat, all);
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const toSend = screensToSend(sections, selection);
  const count = toSend.reduce((n, section) => n + section.screens.length, 0);
  const writable = state === "ready" || state === "creating";

  const send = async (target: ChatInfo | null) => {
    setBusy(true);
    try {
      await sendToChat(chat.workspace_id, target, handoffMessage(chatLabel(chat, all), chat.id, toSend));
    } finally {
      setBusy(false);
    }
  };
  const what =
    selection.length > 0 && count > 0
      ? count === 1
        ? "the selected screen"
        : `the ${count} selected screens`
      : "every screen of this page";
  const them = count === 1 ? "it" : "them";
  const title = !writable
    ? "This workspace takes no more messages"
    : count === 0
      ? "No screen to send yet"
      : source
        ? `Write ${what} into “${chatLabel(source, all)}”'s message box, asking to implement ${them}, and show that chat; nothing is sent`
        : `Write ${what} into another chat's message box, asking to implement ${them}; nothing is sent`;

  const items: MenuItem[] = [
    ...chatsToSendTo(chat, all).map((c) => ({
      label: chatLabel(c, all),
      icon: <MessageSquare size={13} />,
      hint: "Into its message box; nothing is sent",
      onSelect: () => void send(c),
    })),
    ...(chatsToSendTo(chat, all).length > 0 ? (["separator"] as const) : []),
    {
      label: "New chat",
      icon: <MessageSquarePlus size={13} />,
      shortcut: "n",
      hint: "A new chat of this workspace, with the message in its box",
      onSelect: () => void send(null),
    },
  ];

  return (
    <>
      <Button
        size="sm"
        variant="primary"
        className="ml-1"
        disabled={!writable || count === 0}
        busy={busy}
        title={title}
        onClick={(event) => {
          if (source) {
            void send(source);
            return;
          }
          const rect = event.currentTarget.getBoundingClientRect();
          setMenu({ x: rect.left, y: rect.bottom + 4 });
        }}
        data-testid="canvas-send"
      >
        {!busy && <Send size={12} />} {source ? "Send to source chat" : "Send to a chat…"}
      </Button>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={items} onClose={() => setMenu(null)} />}
    </>
  );
}
