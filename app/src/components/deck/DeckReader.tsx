// A slide deck beside its chat (docs/adr/slide-decks.md): numbered thumbnails to reorder,
// duplicate, skip or delete; the current slide, whose texts are retyped by double-click in Edit
// mode and commented on in Comment mode; its speaker notes; Present, Present with notes, the
// versions and the downloads. Every change goes through the daemon (`deck.*`), which writes the
// files, keeps a version and tells the agent at its next message.

import type { ChatInfo, DeckExportFormat, DeckResult, DeckSlide, DeliverableVersion, Mention, PinElement } from "@chauffeur/protocol";
import { Copy, Download, Eye, EyeOff, History, Maximize2, MessageSquare, Minimize2, MonitorPlay, Palette, Play, RefreshCw, Trash2, Type, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { threadsOf } from "@/lib/comments";
import { fitScale, moved, parseReaderMessage, slidePath, step } from "@/lib/deck";
import { deliverableDownloadUrl, type Deliverable } from "@/lib/deliverables";
import { openExternal } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { client, sendNow, toast, useApp } from "@/state/app";
import { CommentComposer, ThreadView } from "../design/CommentsPanel";
import { type ScreenCommenting, ScreenComments } from "../design/ScreenComments";
import { usePins } from "../design/usePins";
import { useFileComments } from "../FileComments";
import { Button, cx, EmptyState, IconButton, Select, Spinner } from "../ui";

type Mode = "view" | "edit" | "comment";
type Draft = { x: number; y: number; element: PinElement | null };

/** The size of an element, followed as it changes; the element may come later than the component. */
function useSize<T extends HTMLElement>(): [(element: T | null) => void, { width: number; height: number }] {
  const [element, setElement] = useState<T | null>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  useEffect(() => {
    if (!element) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry) setSize({ width: entry.contentRect.width, height: entry.contentRect.height });
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, [element]);
  return [setElement, size];
}

/** A slide at its own size, scaled to `scale`; its frame keeps the slide's opaque origin. */
function SlideFrame({ slide, width, height, scale, title, frameRef, onLoad, interactive }: {
  slide: DeckSlide; width: number; height: number; scale: number; title: string;
  frameRef?: (frame: HTMLIFrameElement | null) => void; onLoad?: () => void; interactive: boolean;
}) {
  if (!slide.url) return <div className="flex items-center justify-center bg-raised text-[11px] text-muted" style={{ width: width * scale, height: height * scale }}>{slide.file} is missing</div>;
  return (
    <div className="relative overflow-hidden bg-white" style={{ width: width * scale, height: height * scale }}>
      <iframe
        key={`${slide.url}:${slide.version}`}
        ref={frameRef}
        src={slide.url}
        title={title}
        onLoad={onLoad}
        sandbox="allow-scripts"
        referrerPolicy="no-referrer"
        className={cx("absolute left-0 top-0 origin-top-left border-0", !interactive && "pointer-events-none")}
        style={{ width, height, transform: `scale(${scale})` }}
        tabIndex={interactive ? 0 : -1}
        loading="lazy"
      />
    </div>
  );
}

export function DeckReader({ workspaceId, chat, document, revision, expanded, onExpand, onClose }: {
  workspaceId: string; chat: ChatInfo | null; document: Deliverable; revision: number; expanded: boolean; onExpand: () => void; onClose: () => void;
}) {
  const connection = useApp((s) => s.connection.status);
  const [deck, setDeck] = useState<DeckResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [version, setVersion] = useState<number | null>(null);
  const [versions, setVersions] = useState<DeliverableVersion[]>([]);
  const [at, setAt] = useState(0);
  const [mode, setMode] = useState<Mode>("view");
  const [presenting, setPresenting] = useState(false);
  const [busy, setBusy] = useState(false);
  const readOnly = version != null;
  const params = { workspace: workspaceId, path: document.path };

  // Each change by hand keeps a version: the list is read again after it.
  const [changes, setChanges] = useState(0);
  useEffect(() => {
    if (connection !== "open") return;
    let active = true;
    void client.call("deck.read", { ...params, version: version ?? undefined })
      .then((next) => { if (active) { setDeck(next); setError(null); } })
      .catch((reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceId, document.path, revision, connection, reload, version]);
  useEffect(() => {
    if (connection !== "open") return;
    let active = true;
    void client.call("deliverables.versions", params).then((next) => { if (active) setVersions(next); }).catch(() => {});
    return () => { active = false; };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaceId, document.path, revision, connection, reload, changes]);

  const slides = deck?.slides ?? [];
  const current = slides[Math.min(at, Math.max(0, slides.length - 1))] ?? null;
  const path = current ? slidePath(document.path, current.file) : null;
  const act = useCallback(async (what: string, run: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await run();
    } catch (reason) {
      toast(`Could not ${what}: ${errorMessage(reason)}`, "error");
      setReload((r) => r + 1);
    } finally {
      setBusy(false);
    }
  }, []);
  const change = (what: string, next: Parameters<typeof client.call<"deck.change">>[1]["change"]) =>
    act(what, async () => { setDeck(await client.call("deck.change", { ...params, change: next, chat: chat?.id })); setChanges((n) => n + 1); });

  // The comments of the slide shown, read again after each turn (the agent's answers).
  const comments = useFileComments(workspaceId, readOnly ? null : path, `${revision}:${reload}`);
  const threads = useMemo(() => threadsOf(comments.comments), [comments.comments]);
  const { onFrame, pins, stale, elementUnder } = usePins(threads);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [openThread, setOpenThread] = useState<string | null>(null);
  const [sendToAgent, setSendToAgent] = useState(true);
  const shownThread = threads.find((t) => t.comment.id === openThread) ?? null;
  useEffect(() => { setDraft(null); setOpenThread(null); }, [path]);

  const sendComments = async () => {
    if (!chat || !path) {
      toast("Open the deck's chat to send the comments to its agent", "error");
      return;
    }
    const result = await client.call("files.comments_send", { workspace: workspaceId, path });
    if (result.sent > 0 && (await sendNow(chat, result.message))) toast("Sent to the agent");
  };
  const addComment = (d: Draft, text: string) =>
    act("add the comment", async () => {
      if (!path) return;
      await client.call("files.comment_add", { workspace: workspaceId, path, pin: { x: d.x, y: d.y, element: d.element ?? undefined }, text });
      setDraft(null);
      if (sendToAgent) await sendComments();
      await comments.reload();
    });
  const reply = (threadId: string, text: string, send: boolean) =>
    act("reply", async () => {
      if (!path) return;
      await client.call("files.comment_add", { workspace: workspaceId, path, text, reply_to: threadId });
      if (send) await sendComments();
      await comments.reload();
    });

  // The current slide: its frame, the script's messages, the mode it is in.
  const main = useRef<HTMLIFrameElement | null>(null);
  const presented = useRef<HTMLIFrameElement | null>(null);
  const postMode = useCallback(() => {
    main.current?.contentWindow?.postMessage({ type: "chauffeur:mode", mode: readOnly ? "view" : mode }, "*");
  }, [mode, readOnly]);
  useEffect(postMode, [postMode]);
  const go = useCallback((key: string, present: boolean) => {
    const next = step(slides, at, key, present);
    if (next != null) setAt(next);
  }, [slides, at]);
  useEffect(() => {
    const listener = (event: MessageEvent) => {
      const fromMain = event.source != null && event.source === main.current?.contentWindow;
      const fromPresent = event.source != null && event.source === presented.current?.contentWindow;
      if (!fromMain && !fromPresent) return;
      const message = parseReaderMessage(event.data);
      if (!message) return;
      if (message.kind === "key") {
        if (fromPresent && message.key === "Escape") setPresenting(false);
        else go(message.key, fromPresent);
      } else if (message.kind === "edit" && current && fromMain && !readOnly) {
        if (message.overflow) toast("This text overflows its box: shorten it or ask the agent to resize it");
        void act("save the text", async () => {
          setDeck(await client.call("deck.edit_text", { ...params, slide: current.file, tag: message.tag, index: message.index, before: message.before, html: message.html, chat: chat?.id }));
          setChanges((n) => n + 1);
        });
      }
    };
    window.addEventListener("message", listener);
    return () => window.removeEventListener("message", listener);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [go, current, readOnly, act, chat?.id]);

  const download = async (format: DeckExportFormat) => {
    const base = client.httpUrl(`/deliverables/${encodeURIComponent(workspaceId)}`);
    if (!base) return;
    await act("download the deck", async () => {
      const exported = await client.call("deck.export", { ...params, format, slide: format === "png" ? current?.file : undefined });
      const ticket = await client.call("files.download", { workspace: workspaceId, path: exported.path });
      await openExternal(deliverableDownloadUrl(base, ticket));
    });
  };

  const [box, size] = useSize<HTMLDivElement>();
  const width = deck?.width ?? 1920;
  const height = deck?.height ?? 1080;
  const scale = fitScale(size.width - 32, size.height - 32, width, height);
  const commenting: ScreenCommenting = {
    commentMode: mode === "comment" && !readOnly,
    onToggleCommentMode: () => setMode((m) => (m === "comment" ? "view" : "comment")),
    pins,
    draft: draft && path ? { file: path, x: draft.x, y: draft.y, whole: false, element: draft.element } : null,
    openThread: shownThread,
    onFrame,
    onCommentAt: (file, x, y, _whole, frame) => {
      setOpenThread(null);
      void elementUnder(file, x, y, frame).then((element) => setDraft({ x, y, element }));
    },
    onOpenThread: (id) => { setDraft(null); setOpenThread(id); },
    onEscape: () => { if (draft) setDraft(null); else if (openThread) setOpenThread(null); else setMode("view"); },
    popover: draft ? (
      <div className="w-[300px] rounded-lg border border-line-strong bg-bg p-2 shadow-2xl" data-testid="comment-bubble">
        <div className="mb-1 truncate text-[11px] text-muted">{draft.element?.text ? `On “${draft.element.text}”` : `At ${Math.round(draft.x)}%, ${Math.round(draft.y)}%`}</div>
        <CommentComposer placeholder="Your comment" submitLabel={sendToAgent ? "Send to agent" : "Comment"} busy={busy}
          onSubmit={(text) => void addComment(draft, text)} onCancel={() => setDraft(null)} />
        <label className="mt-1.5 flex items-center gap-1.5 text-[11px] text-muted">
          <input type="checkbox" checked={sendToAgent} onChange={(e) => setSendToAgent(e.target.checked)} data-testid="comment-send-to-agent" />
          Send to the agent, who changes the slide and answers here
        </label>
      </div>
    ) : shownThread && comments.me ? (
      <div className="max-h-[280px] w-[320px] overflow-y-auto rounded-lg border border-line-strong bg-bg p-2.5 shadow-2xl" data-testid="comment-popover">
        <ThreadView thread={shownThread} me={comments.me} busy={busy} stale={stale.has(shownThread.comment.id)}
          onReply={(id, text) => void reply(id, text, false)}
          onReplyAndSend={(id, text) => void reply(id, text, true)}
          onSendThread={() => void act("send the comment", sendComments)}
          onResolve={(id, resolved) => void act("resolve the comment", async () => { await client.call("files.comment_resolve", { workspace: workspaceId, id, resolved }); await comments.reload(); })}
          onEdit={(id, text: string, _mentions: Mention[]) => void act("edit the comment", async () => { await client.call("files.comment_edit", { workspace: workspaceId, id, text }); await comments.reload(); })}
          onDelete={(id) => void act("delete the comment", async () => { await client.call("files.comment_delete", { workspace: workspaceId, id }); setOpenThread(null); await comments.reload(); })}
          onClose={() => setOpenThread(null)} />
      </div>
    ) : null,
  };

  // Arrows when the reader has the focus (the slide's own go through its script).
  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, [contenteditable=true], [data-canvas-popover]")) return;
      if (presenting && event.key === "Escape") { setPresenting(false); return; }
      if (step(slides, at, event.key, presenting) != null && (presenting || target?.closest("[data-deck-reader]"))) {
        event.preventDefault();
        go(event.key, presenting);
      }
    };
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, [slides, at, presenting, go]);

  const [dragging, setDragging] = useState<number | null>(null);
  const versionOptions = [
    { value: "current", label: "Current version" },
    ...versions.map((v) => ({ value: String(v.n), label: `Version ${v.n}`, description: `${v.by === "agent" ? "By the agent" : v.by === "hand" ? "By hand" : "Restored"} · ${new Date(v.at).toLocaleString()}` })),
  ];

  return (
    <section className="flex min-h-0 flex-1 flex-col" aria-label="Deck" data-testid="deck-reader" data-deck-reader>
      <header className="flex h-12 shrink-0 items-center gap-1.5 border-b border-line px-3">
        <MonitorPlay size={15} className="shrink-0 text-muted" />
        <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{deck?.title || document.title}</span>
        {versions.length > 0 && (
          <Select value={version == null ? "current" : String(version)} options={versionOptions} title="Versions" width={260}
            onChange={(value) => setVersion(value === "current" ? null : Number(value))}
            shown={{ label: version == null ? `v${versions[0]?.n ?? 1}` : `v${version}`, icon: <History size={12} /> }} testId="deck-versions" />
        )}
        <IconButton title="Reload" onClick={() => setReload(reload + 1)}><RefreshCw size={14} /></IconButton>
        <IconButton title={expanded ? "Show conversation" : "Expand deck"} onClick={onExpand}>{expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}</IconButton>
        <IconButton title="Close deck" onClick={onClose}><X size={15} /></IconButton>
      </header>
      <div className="flex shrink-0 flex-wrap items-center gap-1.5 border-b border-line px-3 py-2">
        <Button size="sm" variant={mode === "edit" ? "secondary" : "ghost"} aria-pressed={mode === "edit"} disabled={readOnly} onClick={() => setMode(mode === "edit" ? "view" : "edit")} data-testid="deck-edit-mode" title="Double-click a text of the slide to retype it">
          <Type size={13} /> Edit text
        </Button>
        <Button size="sm" variant={mode === "comment" ? "secondary" : "ghost"} aria-pressed={mode === "comment"} disabled={readOnly} onClick={() => setMode(mode === "comment" ? "view" : "comment")} data-testid="deck-comment-mode" title="Click a point of the slide to comment on it">
          <MessageSquare size={13} /> Comment
        </Button>
        <span className="flex-1" />
        <Button size="sm" variant="ghost" disabled={!deck || slides.every((s) => s.skip)} onClick={() => {
          const first = step(slides, Math.max(0, at - 1), "ArrowRight", true);
          if (current?.skip && first != null) setAt(first);
          setPresenting(true);
        }} data-testid="deck-present"><Play size={13} /> Present</Button>
        <Button size="sm" variant="ghost" disabled={!deck || readOnly} title="Opens in your browser: the notes, a clock and the next slide, and the slides' own window for the audience"
          onClick={() => void act("present the deck", async () => openExternal(await client.call("deck.present", params)))}>
          <MonitorPlay size={13} /> With notes
        </Button>
        <Select value={"" as string} title="Download" width={220} disabled={!deck || readOnly || busy}
          shown={{ label: "Download", icon: <Download size={12} /> }}
          options={[{ value: "pdf", label: "PDF", description: "A page per slide" }, { value: "png", label: "This slide as PNG" }, { value: "html", label: "HTML", description: "One page that plays the deck" }]}
          onChange={(value) => void download(value as DeckExportFormat)} testId="deck-download" />
      </div>
      {readOnly && (
        <div className="flex shrink-0 items-center gap-2 border-b border-line bg-raised px-3 py-1.5 text-[12px]">
          <History size={13} className="text-muted" /> Version {version}, read-only.
          <Button size="sm" className="ml-auto" onClick={() => void act("restore this version", async () => {
            await client.call("deliverables.restore", { ...params, n: version! });
            setVersion(null);
            setReload((r) => r + 1);
            toast(`Version ${version} restored as a new version`);
          })} data-testid="deck-restore">Restore</Button>
          <Button size="sm" variant="ghost" onClick={() => setVersion(null)}>Back to current</Button>
        </div>
      )}
      {deck?.brand_changed && !readOnly && (
        <div className="flex shrink-0 items-center gap-2 border-b border-line bg-raised px-3 py-1.5 text-[12px]">
          <Palette size={13} className="text-muted" /> The brand {deck.brand} changed since this deck was made.
          <Button size="sm" className="ml-auto" onClick={() => void act("apply the brand", async () => { setDeck(await client.call("deck.apply_brand", params)); setChanges((n) => n + 1); })}>Apply</Button>
        </div>
      )}
      {error ? <EmptyState title="Could not open this deck"><p>{error}</p><Button onClick={() => setReload(reload + 1)}>Try again</Button></EmptyState>
        : !deck ? <div className="flex flex-1 items-center justify-center gap-2 text-[12px] text-muted" role="status"><Spinner size={14} /> Loading deck…</div>
        : slides.length === 0 ? <EmptyState title="No slides yet">Ask the agent for the deck's slides.</EmptyState>
        : (
          <div className="flex min-h-0 flex-1">
            <ol className="flex w-[148px] shrink-0 flex-col gap-2 overflow-y-auto border-r border-line p-2" aria-label="Slides" data-testid="deck-thumbnails">
              {slides.map((slide, i) => (
                <li key={slide.file}
                  draggable={!readOnly}
                  onDragStart={() => setDragging(i)}
                  onDragOver={(event) => { if (dragging != null) event.preventDefault(); }}
                  onDrop={() => {
                    if (dragging == null || dragging === i) return;
                    const files = moved(slides.map((s) => s.file), dragging, dragging < i ? i + 1 : i);
                    setDragging(null);
                    void change("reorder the slides", { op: "reorder", files });
                  }}
                  onDragEnd={() => setDragging(null)}
                  className={cx("group relative cursor-pointer rounded-md p-1", i === at ? "bg-hover ring-1 ring-accent" : "hover:bg-hover", slide.skip && "opacity-50")}
                  onClick={() => setAt(i)}
                  data-testid="deck-thumbnail"
                >
                  <div className="mb-1 flex items-center gap-1 text-[10.5px] text-muted">
                    <span className="font-semibold">{i + 1}</span>
                    <span className="min-w-0 flex-1 truncate">{slide.title}</span>
                    {slide.skip && <EyeOff size={10} />}
                  </div>
                  <SlideFrame slide={slide} width={width} height={height} scale={128 / width} title={`Slide ${i + 1}`} interactive={false} />
                  {!readOnly && (
                    <div className="absolute right-1 top-5 hidden gap-0.5 rounded bg-bg/90 p-0.5 shadow group-hover:flex" onClick={(event) => event.stopPropagation()}>
                      <IconButton className="h-5 w-5" title="Duplicate" onClick={() => void change("duplicate the slide", { op: "duplicate", file: slide.file })}><Copy size={11} /></IconButton>
                      <IconButton className="h-5 w-5" title={slide.skip ? "Show in Present and downloads" : "Skip in Present and downloads"} onClick={() => void change("skip the slide", { op: "skip", file: slide.file, skip: !slide.skip })}>{slide.skip ? <Eye size={11} /> : <EyeOff size={11} />}</IconButton>
                      <IconButton className="h-5 w-5" title="Delete" onClick={() => {
                        if (!window.confirm(`Delete slide ${i + 1}? It stays in the deck's versions.`)) return;
                        setAt(Math.max(0, Math.min(at, slides.length - 2)));
                        void change("delete the slide", { op: "delete", file: slide.file });
                      }}><Trash2 size={11} /></IconButton>
                    </div>
                  )}
                </li>
              ))}
            </ol>
            <div className="flex min-h-0 min-w-0 flex-1 flex-col">
              <div ref={box} className="relative flex min-h-0 flex-1 items-center justify-center overflow-hidden bg-raised" data-testid="deck-stage">
                {current && scale > 0 && (
                  <div className="relative shadow-lg">
                    <SlideFrame slide={current} width={width} height={height} scale={scale} title={`Slide ${at + 1}`} interactive={mode !== "comment"}
                      frameRef={(frame) => { main.current = frame; if (path) onFrame(path, frame, false); }}
                      onLoad={() => { postMode(); if (path) onFrame(path, main.current, true); }} />
                    {path && !readOnly && <ScreenComments file={path} width={width * scale} height={height * scale} frame={() => main.current} commenting={commenting} />}
                  </div>
                )}
              </div>
              {current && <Notes key={`${current.file}:${current.notes}:${version}`} slide={current} readOnly={readOnly}
                onSave={(notes) => void change("save the notes", { op: "notes", file: current.file, notes })} />}
              <footer className="shrink-0 border-t border-line px-3 py-1.5 text-[11px] text-muted">
                {mode === "edit" ? "Double-click a text to retype it; ⌘⏎ or a click outside saves, Esc cancels." : mode === "comment" ? "Click a point of the slide to comment on it." : `Slide ${at + 1} of ${slides.length}. Ask for changes in this conversation.`}
              </footer>
            </div>
          </div>
        )}
      {presenting && deck && current && (
        <Presenting deck={deck} at={at} frameRef={(frame) => { presented.current = frame; }} onNext={() => go("ArrowRight", true)} onClose={() => setPresenting(false)} />
      )}
    </section>
  );
}

function Notes({ slide, readOnly, onSave }: { slide: DeckSlide; readOnly: boolean; onSave: (notes: string) => void }) {
  const [notes, setNotes] = useState(slide.notes);
  return (
    <textarea
      value={notes}
      readOnly={readOnly}
      onChange={(event) => setNotes(event.target.value)}
      onBlur={() => { if (notes !== slide.notes) onSave(notes); }}
      placeholder={readOnly ? "No speaker notes" : "Speaker notes"}
      rows={3}
      className="h-20 shrink-0 resize-none border-t border-line bg-panel px-3 py-2 text-[12.5px] text-fg outline-none placeholder:text-faint"
      aria-label="Speaker notes"
      data-testid="deck-notes"
    />
  );
}

/** The deck full screen: the current slide on black, scaled to the window; Esc leaves. */
function Presenting({ deck, at, frameRef, onNext, onClose }: {
  deck: DeckResult; at: number; frameRef: (frame: HTMLIFrameElement | null) => void; onNext: () => void; onClose: () => void;
}) {
  const [box, size] = useSize<HTMLDivElement>();
  useEffect(() => {
    void window.document.documentElement.requestFullscreen?.().catch(() => {});
    return () => { if (window.document.fullscreenElement) void window.document.exitFullscreen().catch(() => {}); };
  }, []);
  const slide = deck.slides[at];
  const scale = Math.min(size.width / deck.width, size.height / deck.height);
  return (
    <div ref={box} className="fixed inset-0 z-50 flex items-center justify-center bg-black" data-testid="deck-presenting" onClick={onNext}>
      {slide && scale > 0 && <SlideFrame slide={slide} width={deck.width} height={deck.height} scale={scale} title={`Slide ${at + 1}`} interactive frameRef={frameRef} />}
      <button type="button" className="absolute right-3 top-3 rounded bg-white/10 px-2 py-1 text-[12px] text-white/80 hover:bg-white/20" onClick={(event) => { event.stopPropagation(); onClose(); }}>Esc</button>
    </div>
  );
}
