import type { CanvasResult, CommentAuthor, DesignComment, FromReviewer, PinElement } from "@chauffeur/protocol";
import { ArrowLeft, Check, ChevronRight, Copy, Layers2, LockKeyhole, MessageSquare, MessageSquarePlus, RotateCcw, X, ZoomIn } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type MentionSetup, MentionText, useMentionPicker } from "@/components/design/Mentions";
import { ScreenFrame } from "@/components/design/ScreenFrame";
import { usePins } from "@/components/design/usePins";
import { placeOf, threadsOf, type Draft } from "@/lib/comments";
import { initials, liveLabel } from "@/lib/sharePage";
import { relativeTime } from "@/lib/format";

type Props = {
  id: string; title: string; owner: string; me: CommentAuthor; canvas: CanvasResult;
  comments: DesignComment[]; online: boolean; lastPushAt?: number; connected: boolean;
  say: (message: FromReviewer) => boolean; serviceError: string | null; snapshotEpoch: number; onReconnect: () => void;
  pins: ReturnType<typeof usePins>["pins"]; stale: Set<string>;
  onFrame: ReturnType<typeof usePins>["onFrame"];
  elementUnder: (file: string, x: number, y: number, frame?: HTMLIFrameElement | null) => Promise<PinElement | null>;
  /** Who a comment may mention (docs/adr/mentions-in-share-comments.md). */
  mentionSetup?: MentionSetup;
  /** The threads mentioning the reader that they have not opened. */
  unreadThreads?: readonly string[];
  /** The reader opened a thread: its mentions of them are read. */
  onSeen?: (thread: string) => void;
};
type SavedDraft = { anchor: Draft | null; text: string; reply: string; thread: string | null; sent?: { text: string; screen: string; reply: string | null; before: string[] } };
type Pending = { text: string; screen: string; reply: string | null; before: Set<string>; epoch: number };

function readDraft(key: string): SavedDraft | null {
  try {
    const value: unknown = JSON.parse(sessionStorage.getItem(key) ?? "null");
    if (!value || typeof value !== "object" || !("text" in value) || typeof value.text !== "string" || !("reply" in value) || typeof value.reply !== "string") return null;
    // This is our own session-only draft, never content from a screen's sandbox.
    const draft = value as SavedDraft;
    if (draft.anchor && (typeof draft.anchor.file !== "string" || typeof draft.anchor.x !== "number" || typeof draft.anchor.y !== "number" || typeof draft.anchor.whole !== "boolean")) return null;
    return draft;
  } catch { return null; }
}

export function MobileReview({ id, title, owner, me, canvas, comments, online, lastPushAt, connected, say, serviceError, snapshotEpoch, onReconnect, pins, stale, onFrame, elementUnder, mentionSetup, unreadThreads = [], onSeen }: Props) {
  const initial = new URLSearchParams(location.search);
  const initialThread = initial.get("thread");
  const initialFile = initial.get("screen") ?? comments.find(comment => comment.id === initialThread)?.screen ?? canvas.pages.flatMap(page => page.sections.flatMap(section => section.screens))[0]?.file ?? canvas.unsorted[0]?.file;
  const keyFor = (file: string | undefined, thread: string | null) => `chauffeur:review-draft:${id}:${me.email}:${encodeURIComponent(file ?? "")}:${thread ?? "screen"}`;
  const [storageKey, setStorageKey] = useState(() => keyFor(initialFile, initialThread));
  const saved = useMemo(() => readDraft(storageKey), [storageKey]);
  const [target, setTarget] = useState(() => new URLSearchParams(location.search).get("screen"));
  const [threadId, setThreadId] = useState(() => initialThread);
  const [view, setView] = useState<"screen" | "screens" | "comments">("screen");
  const [filter, setFilter] = useState<"open" | "resolved">("open");
  const [mode, setMode] = useState(false);
  const [draft, setDraft] = useState<Draft | null>(saved?.anchor ?? null);
  const [text, setText] = useState(saved?.text ?? "");
  const [reply, setReply] = useState(saved?.reply ?? "");
  const [pending, setPending] = useState<Pending | null>(() => saved?.sent ? { ...saved.sent, before: new Set(saved.sent.before), epoch: snapshotEpoch } : null);
  const [error, setError] = useState<string | null>(null);
  const [posted, setPosted] = useState(false);
  const [confirm, setConfirm] = useState<(() => void) | null>(null);
  const [zoomed, setZoomed] = useState(false);
  const frame = useRef<HTMLIFrameElement | null>(null);
  const textBox = useRef<HTMLTextAreaElement | null>(null);
  const replyBox = useRef<HTMLTextAreaElement | null>(null);
  const textPicker = useMentionPicker(mentionSetup, text, setText, textBox);
  const replyPicker = useMentionPicker(mentionSetup, reply, setReply, replyBox);
  const stage = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ width: 370, height: 620 });
  const screens = useMemo(() => [...canvas.pages.flatMap(page => page.sections.flatMap(section => section.screens.map(screen => ({ screen, section: section.title })))), ...canvas.unsorted.map(screen => ({ screen, section: "Unsorted" }))], [canvas]);
  const threads = useMemo(() => threadsOf(comments), [comments]);
  const thread = threads.find(item => item.comment.id === threadId);
  const file = target ?? thread?.comment.screen ?? screens[0]?.screen.file;
  const current = screens.find(item => item.screen.file === file);
  const screen = current?.screen;
  const screenThreads = threads.filter(item => item.comment.screen === file);
  const open = screenThreads.filter(item => !item.comment.resolved);
  const resolved = screenThreads.filter(item => item.comment.resolved);
  const missing = !current || (threadId != null && (!thread || thread.comment.screen !== file));
  const trackFrame = useCallback((value: HTMLIFrameElement | null, loaded: boolean) => { frame.current = value; if (file) onFrame(file, value, loaded); }, [file, onFrame]);

  useEffect(() => {
    if (!stage.current) return;
    const observer = new ResizeObserver(entries => {
      const rect = entries[0]?.contentRect;
      if (rect) setSize({ width: Math.max(1, rect.width - 20), height: Math.max(1, rect.height - 20) });
    });
    observer.observe(stage.current);
    return () => observer.disconnect();
  }, [view, missing]);
  useEffect(() => {
    const value: SavedDraft = { anchor: draft, text, reply, thread: threadId, sent: pending ? { text: pending.text, screen: pending.screen, reply: pending.reply, before: [...pending.before] } : undefined };
    try {
      if (text.trim() || reply.trim()) sessionStorage.setItem(storageKey, JSON.stringify(value));
      else sessionStorage.removeItem(storageKey);
    } catch { /* Browser storage may be disabled; the draft remains in memory. */ }
  }, [draft, text, reply, threadId, storageKey, pending]);
  useEffect(() => {
    if (!text.trim() && !reply.trim()) return;
    const warn = (event: BeforeUnloadEvent) => { event.preventDefault(); };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, [text, reply]);
  useEffect(() => {
    if (!pending) return;
    const receipt = comments.find(comment => !pending.before.has(comment.id) && comment.author.email === me.email && comment.text === pending.text && comment.screen === pending.screen && (comment.reply_to ?? null) === pending.reply);
    if (!receipt) {
      if (snapshotEpoch > pending.epoch) { setPending(null); setError("This comment was not found after reconnecting. Your draft is kept; retry when ready."); }
      return;
    }
    setPending(null); setError(null); setPosted(true);
    if (pending.reply) setReply("");
    else { setText(""); setDraft(null); setThreadId(receipt.id); try { sessionStorage.removeItem(storageKey); } catch { /* The in-memory draft was cleared. */ } setStorageKey(keyFor(receipt.screen, receipt.id)); updateTarget(receipt.screen, receipt.id); }
  }, [comments, pending, me.email, snapshotEpoch]);
  useEffect(() => {
    if (!pending) return;
    const timer = setTimeout(() => setError("The service has not confirmed this comment. Your draft is kept. Reconnect before retrying to check whether it was posted."), 12_000);
    return () => clearTimeout(timer);
  }, [pending]);
  useEffect(() => { if (serviceError) { setError(serviceError); setPending(null); } }, [serviceError]);
  useEffect(() => { if (threadId && unreadThreads.includes(threadId)) onSeen?.(threadId); }, [threadId, unreadThreads, onSeen]);
  const safely = (action: () => void) => {
    if (text.trim() || reply.trim() || pending) setConfirm(() => action);
    else action();
  };
  const closePanel = () => safely(() => { setDraft(null); setText(""); setReply(""); setThreadId(null); setPosted(false); updateTarget(file, null); });
  const back = () => {
    if (draft || threadId) closePanel();
    else if (view !== "screen") setView("screen");
    else safely(() => { location.assign("/mobile-close"); });
  };
  const backRef = useRef(back); backRef.current = back;
  useEffect(() => {
    const listener = (event: Event) => { event.preventDefault(); backRef.current(); };
    const keyboard = (event: KeyboardEvent) => { if (event.key === "Escape") { event.preventDefault(); backRef.current(); } };
    window.addEventListener("chauffeur:back", listener);
    window.addEventListener("keydown", keyboard);
    return () => { window.removeEventListener("chauffeur:back", listener); window.removeEventListener("keydown", keyboard); };
  }, []);
  function updateTarget(nextFile: string | undefined, nextThread: string | null) {
    const url = new URL(location.href);
    if (nextFile) url.searchParams.set("screen", nextFile); else url.searchParams.delete("screen");
    if (nextThread) url.searchParams.set("thread", nextThread); else url.searchParams.delete("thread");
    history.replaceState(null, "", url);
  }
  const choose = (next: string, nextThread: string | null = null) => safely(() => {
    const nextKey = keyFor(next, nextThread);
    const kept = readDraft(nextKey);
    setStorageKey(nextKey); setTarget(next); setThreadId(nextThread);
    setDraft(!nextThread && kept?.anchor?.file === next ? kept.anchor : null);
    setText(!nextThread ? kept?.text ?? "" : ""); setReply(nextThread ? kept?.reply ?? "" : "");
    setView("screen"); setMode(false); setPosted(false); setError(null); updateTarget(next, nextThread);
  });
  const start = (anchor: Draft) => safely(() => {
    setDraft(anchor); setText(""); setThreadId(null); setPosted(false); setError(null); setView("screen");
    if (!anchor.whole) void elementUnder(anchor.file, anchor.x, anchor.y, frame.current).then(element => setDraft(value => value?.file === anchor.file && value.x === anchor.x && value.y === anchor.y ? { ...value, element } : value));
  });
  const send = (isReply: boolean) => {
    const body = (isReply ? reply : text).trim();
    if (!body || !connected || pending || !file) return;
    const mentions = (isReply ? replyPicker : textPicker).mentions();
    const named = mentions.length > 0 ? mentions : undefined;
    const message: FromReviewer = isReply && thread ? { type: "add", screen: "", reply_to: thread.comment.id, text: body, mentions: named } : { type: "add", screen: draft?.file ?? file, pin: draft && !draft.whole ? { x: draft.x, y: draft.y, element: draft.element ?? undefined } : undefined, text: body, mentions: named };
    if (say(message)) { setError(null); setPending({ text: body, screen: file, reply: isReply ? threadId : null, epoch: snapshotEpoch, before: new Set(comments.map(comment => comment.id)) }); }
  };
  const copy = async () => {
    try { const url = new URL(location.href); url.searchParams.delete("native"); await navigator.clipboard.writeText(url.href); setError("Link copied. Access remains limited to the design’s reviewers."); }
    catch { setError("Could not copy this thread’s link. Copy the address from your browser instead."); }
  };
  const scale = screen ? zoomed ? 1 : Math.min(size.width / screen.width, draft || thread ? 1 : size.height / screen.height, 1) : 1;
  useEffect(() => {
    const viewport = stage.current;
    if (!viewport || (!draft && !thread) || !screen) return;
    const y = (draft?.y ?? thread?.comment.pin?.y ?? 0) / 100 * screen.height * scale;
    viewport.scrollTop = Math.max(0, y - viewport.clientHeight * 0.4);
  }, [draft?.y, thread?.comment.id, screen, scale, view]);
  const draftNumber = threads.length + 1;
  const openApp = () => { const url = new URL(location.href); url.searchParams.delete("native"); url.hash = ""; location.assign(`chauffeur://review?url=${encodeURIComponent(url.href)}`); };
  return <div className="mobile-review" data-testid="mobile-review">
    <header className="review-header"><button aria-label="Back" onClick={back}><ArrowLeft /></button><div className="review-grow"><h1>{view === "screens" ? title : view === "comments" ? "Comments" : screen?.label || screen?.file || "Shared design"}</h1><div className="review-sub">{view === "screens" ? `Shared by ${owner} · ${screens.length} screens` : `${title}${view === "comments" ? ` / ${screen?.label || file || ""}` : ` · shared by ${owner}`}`}</div></div>{new URLSearchParams(location.search).get("native") !== "1" && <button onClick={openApp} aria-label="Open in Chauffeur"><ChevronRight /></button>}</header>
    <div className="review-status"><span className={`review-dot ${connected && online ? "live" : ""}`} /><span>{connected ? liveLabel(online, lastPushAt).text : "You’re offline · Reconnecting…"}</span><span className="review-grow" /><LockKeyhole size={13} /> Comment access <span className="review-avatar">{initials(me.name)}</span></div>
    {!online && connected && <div className="review-banner">The owner’s Mac is offline. You can still comment on the shared design.</div>}
    {unreadThreads.length > 0 && !threadId && !draft && (() => { const next = threads.find(item => item.comment.id === unreadThreads[0]); return next ? <button className="review-banner review-mentions" onClick={() => choose(next.comment.screen, next.comment.id)} data-testid="mobile-mentions">{unreadThreads.length === 1 ? "1 mention for you" : `${unreadThreads.length} mentions for you`} · Open</button> : null; })()}
    {view === "screens" ? <main className="review-list">
      {screens.length === 0 && <div className="review-empty"><h2>No screens yet</h2><p>The owner has not shared any screens yet.</p></div>}
      {screens.map(({ screen: item, section }, index) => <div key={`${section}:${item.file}:${index}`}>{index === 0 || screens[index - 1].section !== section ? <h2 className="review-section">{section}</h2> : null}<button className="review-screen-row" aria-label={item.label || item.file} onClick={() => choose(item.file)}><span className="review-screen-thumb"><span style={{ transform: `scale(${Math.min(36 / item.width, 64 / item.height)})`, transformOrigin: "top left", display: "block" }}><ScreenFrame screen={item} interactive={false} /></span></span><div className="review-grow"><strong>{item.label || item.file}</strong><div className="review-sub">{threads.filter(t => t.comment.screen === item.file && !t.comment.resolved).length || "No"} open comments</div></div><ChevronRight /></button></div>)}
    </main> : missing ? <main className="review-empty"><LockKeyhole size={30} /><h2>{threadId && !thread ? "This comment is no longer available" : "This screen is no longer available"}</h2><p>{title} is still shared. Choose another screen from the design.</p><button className="review-primary" onClick={() => setView("screens")}>View design screens</button></main> : view === "comments" ? <>
      <div className="review-filters"><button className={filter === "open" ? "active" : ""} onClick={() => setFilter("open")}>Open <span>{open.length}</span></button><button className={filter === "resolved" ? "active" : ""} onClick={() => setFilter("resolved")}>Resolved <span>{resolved.length}</span></button></div>
      <main className="review-list">{(filter === "open" ? open : resolved).length === 0 ? <div className="review-empty"><h2>No {filter} comments</h2><p>Use Comment to leave feedback on a specific part of this screen.</p></div> : <><h2 className="review-section">{screen?.label || file}</h2>{(filter === "open" ? open : resolved).map(item => <button className="review-comment-row" key={item.comment.id} onClick={() => choose(item.comment.screen, item.comment.id)}><div className="review-person"><span className="review-number">{item.number}</span><span className="review-avatar">{initials(item.comment.author.name)}</span><strong>{item.comment.author.name}</strong><time>{relativeTime(new Date(item.comment.at).toISOString())}</time></div><p><MentionText text={item.comment.text} mentions={item.comment.mentions} me={me.email} /></p><div className="review-sub">{item.comment.resolved ? `Resolved · ${item.comment.resolved.by}` : `${item.replies.length} ${item.replies.length === 1 ? "reply" : "replies"} · ${placeOf(item.comment)}`}</div></button>)}</>}</main>
      <footer className="review-tools"><button className="review-primary" onClick={() => { setView("screen"); setMode(true); }}>Add a comment</button></footer>
    </> : <>
      {mode && !draft && !thread && <div className="review-hint">Tap the design to place a comment <button onClick={() => start({ file: file!, x: 0, y: 0, whole: true, element: null })}>Whole screen</button></div>}
      <main className={`review-main ${draft || thread ? "has-feedback" : ""}`}>
        <div className={`review-stage ${zoomed ? "zoomed" : ""}`} ref={stage}><div className="review-artboard" style={{ width: screen!.width * scale, height: screen!.height * scale }}><div style={{ width: screen!.width, height: screen!.height, transform: `scale(${scale})`, transformOrigin: "0 0" }}><ScreenFrame screen={screen!} interactive={!mode} capture onFrame={trackFrame} /></div>
          {mode && <button className="review-comment-cover" aria-label="Place a comment on the screen" onClick={event => { const rect = event.currentTarget.getBoundingClientRect(); start({ file: file!, x: Math.max(0, Math.min(100, (event.clientX - rect.left) / rect.width * 100)), y: Math.max(0, Math.min(100, (event.clientY - rect.top) / rect.height * 100)), whole: false, element: null }); }} />}
          {(pins[file!] ?? []).map(pin => <button className={`review-pin ${pin.stale ? "stale" : ""}`} key={pin.thread.comment.id} style={{ left: pin.whole ? 22 : Math.max(22, Math.min(screen!.width * scale - 22, screen!.width * scale * pin.x / 100)), top: pin.whole ? 22 : Math.max(22, Math.min(screen!.height * scale - 22, screen!.height * scale * pin.y / 100)) }} onClick={() => choose(file!, pin.thread.comment.id)} aria-label={`Comment ${pin.thread.number}`}><span>{pin.thread.number}</span></button>)}
          {draft && <span className="review-pin new" style={{ left: draft.whole ? 22 : Math.max(22, Math.min(screen!.width * scale - 22, screen!.width * scale * draft.x / 100)), top: draft.whole ? 22 : Math.max(22, Math.min(screen!.height * scale - 22, screen!.height * scale * draft.y / 100)) }}><span>{draftNumber}</span></span>}
        </div></div>
        {draft ? <section className="review-feedback" aria-label="New comment"><div className="review-feedback-head"><h2>New comment</h2><button aria-label="Cancel comment" onClick={closePanel}><X /></button></div><div className="review-feedback-content"><div className="review-anchor"><span className="review-number">{draftNumber}</span>{screen?.label || file} · {draft.whole ? "Whole screen" : draft.element?.text || "Screen location"}</div><textarea ref={textBox} aria-label="Your comment" placeholder={mentionSetup ? "Leave a comment… (@ to mention)" : "Leave a comment…"} value={text} disabled={!!pending} onChange={event => setText(event.target.value)} onKeyDown={event => { textPicker.onKeyDown(event); }} onSelect={textPicker.track} />{textPicker.list}{error && <p className="review-error" role="status">{error}{pending && <button onClick={onReconnect}>Check connection</button>}</p>}</div><div className="review-feedback-foot"><span className="review-sub">{pending ? "Posting comment…" : "Visible to the design’s reviewers"}</span><button className="review-primary" disabled={!text.trim() || !connected || !!pending} onClick={() => send(false)}>{pending ? "Posting…" : error ? "Retry" : "Post comment"}</button></div></section> : thread ? <section className="review-feedback" aria-label="Comment thread"><div className="review-feedback-head"><h2>{thread.comment.resolved ? "Resolved thread" : posted ? "Comment posted" : `Comment ${thread.number}`}</h2><button aria-label="Close thread" onClick={closePanel}><X /></button></div><div className="review-feedback-content"><div className="review-anchor"><span className="review-number">{thread.number}</span>{screen?.label || file} · {placeOf(thread.comment)}</div>{stale.has(thread.comment.id) && <p className="review-stale">This element changed. The original pin position and text are kept.</p>}{[thread.comment, ...thread.replies].map(comment => <div key={comment.id}><div className="review-person"><span className="review-avatar">{initials(comment.author.name)}</span><strong>{comment.author.name}</strong><time>{relativeTime(new Date(comment.at).toISOString())}</time></div><p className="review-thread-text"><MentionText text={comment.text} mentions={comment.mentions} me={me.email} /></p></div>)}<div className="review-reply"><textarea ref={replyBox} aria-label="Reply" placeholder="Reply…" value={reply} disabled={!!pending} onChange={event => setReply(event.target.value)} onKeyDown={event => { replyPicker.onKeyDown(event); }} onSelect={replyPicker.track} />{replyPicker.list}<button className="review-primary" disabled={!reply.trim() || !connected || !!pending} onClick={() => send(true)}>{pending ? "Posting…" : "Reply"}</button></div>{error && <p className="review-error" role="status">{error}{pending && <button onClick={onReconnect}>Check connection</button>}</p>}</div><div className="review-feedback-foot"><button onClick={() => void copy()}><Copy size={16} /> Copy link</button><button disabled={!connected || !!pending} onClick={() => { say({ type: "resolve", id: thread.comment.id, resolved: !thread.comment.resolved }); }}><Check size={16} />{thread.comment.resolved ? "Reopen" : "Resolve"}</button></div></section> : null}
      </main>
      <footer className="review-tools"><button onClick={() => safely(() => setView("screens"))}><Layers2 /> Screens</button><button className={mode ? "active" : ""} aria-pressed={mode} onClick={() => safely(() => { setMode(!mode); setDraft(null); setText(""); setThreadId(null); })}><MessageSquarePlus /> Comment</button><button aria-label={`${open.length} open comments`} onClick={() => safely(() => setView("comments"))}><MessageSquare /> {open.length}</button><button aria-label={zoomed ? "Fit screen" : "Zoom screen"} onClick={() => setZoomed(!zoomed)}><ZoomIn /></button></footer>
    </>}
    {!connected && <div className="review-connection" role="status">Your draft is kept. <button onClick={onReconnect}><RotateCcw size={14} /> Retry connection</button></div>}
    {confirm && <div className="review-modal"><section role="dialog" aria-modal="true" aria-label="Discard this comment?"><h2>Discard this comment?</h2><p>This comment has not been confirmed as posted.</p><button className="review-primary" onClick={() => { const action = confirm; try { sessionStorage.removeItem(storageKey); } catch { /* Keep editing remains available when storage is disabled. */ } setConfirm(null); setText(""); setReply(""); setDraft(null); setPending(null); action(); }}>Discard comment</button><button onClick={() => setConfirm(null)}>Keep editing</button></section></div>}
  </div>;
}
