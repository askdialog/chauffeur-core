// The guest page of a workspace share (story 53): the workspace's chats as the app shows them,
// read-only and live over the guest socket, and the markdown of its notes. A guest who can suggest
// writes a message under a chat: it goes to the owner, who sends it or not, and every guest sees
// what became of it; they take it back while it waits. They also select a passage of the agent's
// message to comment on it, or reply to a comment, for the owner and the guests, mentioning any of
// them with `@` (docs/adr/mentions-in-share-comments.md); a comment that mentions the reader stands
// out until its thread shows. Nothing on the page acts on a chat: requests wait for the owner.

import type { CommentAuthor, EventEnvelope, FromGuest, Mention, ShareComment, SharedChat, Suggestion, ToGuest, WorkspaceShareSnapshot } from "@chauffeur/protocol";
import { AtSign, FileText, LogOut, MessageSquarePlus, PenTool, Trash2, Users } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { groupActivity } from "@/lib/chat";
import { commentParam, newMentions } from "@/lib/mentions";
import { placeComments } from "@/lib/shareComments";
import { initials, liveLabel } from "@/lib/sharePage";
import { canDeleteSuggestion, suggestionStateLabel } from "@/lib/workspaceShare";
import {
  chatTitle,
  type GuestState,
  guestChatStatus,
  guestSocketUrl,
  guestState,
  loaded,
  loadFailed,
  reduceGuest,
  seeThread,
  startLoading,
  unreadByChat,
} from "@/lib/workspaceSharePage";
import { relativeTime } from "@/lib/format";
import { ActivityContext, ChatRowView } from "@/components/chat/ChatItems";
import { ChatHostContext, NO_HOST } from "@/components/chat/chatHost";
import { CommentSelectionButton, type Passage, PassageCommentBox, selectedPassage, ShareCommentsContext } from "@/components/chat/ShareComments";
import type { MentionSetup } from "@/components/design/Mentions";
import { Markdown } from "@/components/Markdown";
import { Badge, Button, cx, Spinner } from "@/components/ui";
import { api, ServiceError } from "./api";
import { type GateWords, PeoplePanel, ShareGate } from "./SharePage";
import icon from "./icon.png";

const RECONNECT_MIN_MS = 1000;
const RECONNECT_MAX_MS = 15_000;
const SUGGESTION_MAX = 20_000;

const WORDS: GateWords = { brand: "Workspace", kind: "workspace" };

type Shown = { kind: "chat"; id: string } | { kind: "note"; path: string };

export function WorkspaceSharePage({ id }: { id: string }) {
  return (
    <ShareGate id={id} load={api.workspaceSnapshot} words={WORDS}>
      {(open) => <OpenWorkspace {...open} />}
    </ShareGate>
  );
}

function OpenWorkspace({
  id,
  me,
  first,
  onRefused,
  onSignedOut,
}: {
  id: string;
  me: CommentAuthor;
  first: WorkspaceShareSnapshot;
  onRefused: (text: string) => void;
  onSignedOut: () => void;
}) {
  const [state, setState] = useState<GuestState>(() => guestState(first));
  const stateRef = useRef(state);
  stateRef.current = state;
  const [connected, setConnected] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  // `?chat=<id>&comment=<id>`: the link of a mention opens its chat on its thread.
  const [linked] = useState(() => commentParam(location.search));
  const [shown, setShown] = useState<Shown | null>(() => {
    const chat = first.chats.find((c) => c.id === linked?.chat) ?? first.chats[0];
    return chat ? { kind: "chat", id: chat.id } : null;
  });
  const [target, setTarget] = useState<string | null>(() => linked?.comment ?? null);
  const [peopleOpen, setPeopleOpen] = useState(false);
  const socket = useRef<WebSocket | null>(null);
  const snapshot = state.snapshot;
  const [, tick] = useState(0);
  // "Last update 2h ago" moves with the clock.
  useEffect(() => {
    const timer = setInterval(() => tick((n) => n + 1), 30_000);
    return () => clearInterval(timer);
  }, []);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(null), 5000);
    return () => clearTimeout(timer);
  }, [notice]);

  // The guest socket: the snapshot again at each (re)connection, then what changes; reconnects
  // with backoff until the share refuses the guest.
  useEffect(() => {
    let closed = false;
    let delay = RECONNECT_MIN_MS;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const connect = () => {
      const ws = new WebSocket(guestSocketUrl(location, id));
      socket.current = ws;
      ws.onopen = () => {
        setConnected(true);
        delay = RECONNECT_MIN_MS;
      };
      ws.onmessage = (event) => {
        let message: ToGuest;
        try {
          message = JSON.parse(String(event.data)) as ToGuest;
        } catch {
          return;
        }
        if (message.type === "comments") {
          const named = newMentions(stateRef.current.snapshot.comments, message.comments, me.email);
          if (named[0]) setNotice(`${named[0].author.name} mentioned you`);
        }
        if (message.type === "error") setNotice(message.reason);
        else if (message.type === "refused") {
          closed = true;
          onRefused(message.reason);
        } else setState((current) => reduceGuest(current, message));
      };
      ws.onclose = () => {
        setConnected(false);
        socket.current = null;
        if (closed) return;
        timer = setTimeout(connect, delay);
        delay = Math.min(delay * 2, RECONNECT_MAX_MS);
      };
    };
    connect();
    return () => {
      closed = true;
      if (timer) clearTimeout(timer);
      socket.current?.close();
    };
  }, [id, me.email, onRefused]);

  // The chat shown is read when it has no view: at first, after its history was replaced, after a
  // reconnection.
  const shownChat = shown?.kind === "chat" ? shown.id : null;
  const needsRead = shownChat != null && !state.views[shownChat] && !state.loading[shownChat];
  const generation = shownChat ? (state.generations[shownChat] ?? 0) : 0;
  useEffect(() => {
    if (!shownChat || !needsRead) return;
    setState((current) => startLoading(current, shownChat).state);
    api.chatEvents(id, shownChat).then(
      (envelopes: EventEnvelope[]) => setState((current) => loaded(current, shownChat, generation, envelopes)),
      (error: unknown) => {
        setState((current) => loadFailed(current, shownChat));
        if (error instanceof ServiceError && error.status === 401) onSignedOut();
        else if (error instanceof ServiceError && [403, 410].includes(error.status)) onRefused(error.message);
        else setNotice(`Could not read the chat: ${error instanceof Error ? error.message : String(error)}`);
      },
    );
  }, [id, shownChat, needsRead, generation, onRefused, onSignedOut]);

  // A chat that left the list (closed or deleted by the owner) leaves the page too.
  useEffect(() => {
    if (shown?.kind === "chat" && !snapshot.chats.some((c) => c.id === shown.id)) {
      setShown(snapshot.chats[0] ? { kind: "chat", id: snapshot.chats[0].id } : null);
    }
    if (shown?.kind === "note" && !snapshot.notes.some((n) => n.path === shown.path)) setShown(null);
  }, [shown, snapshot.chats, snapshot.notes]);

  // Esc closes "Shared with" wherever the focus is.
  useEffect(() => {
    if (!peopleOpen) return;
    const listener = (event: KeyboardEvent) => {
      if (event.key === "Escape") setPeopleOpen(false);
    };
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, [peopleOpen]);

  /** `quiet`: no notice when not connected (a thread seen is said again when it shows next). */
  const say = useCallback((message: FromGuest, quiet = false): boolean => {
    const ws = socket.current;
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      if (!quiet) setNotice("Not connected to the share: nothing was sent. It reconnects on its own.");
      return false;
    }
    ws.send(JSON.stringify(message));
    return true;
  }, []);
  const markSeen = useCallback(
    (thread: string) => {
      if (say({ type: "seen", thread }, true)) setState((current) => seeThread(current, thread));
    },
    [say],
  );
  // Who a comment may mention: the owner and the guests; only the owner gives access.
  const mentionSetup = useMemo<MentionSetup>(
    () => ({ people: snapshot.people, me: me.email, teamDomain: snapshot.team_domain, owner: snapshot.owner.name }),
    [snapshot.people, snapshot.team_domain, snapshot.owner.name, me.email],
  );
  const unread = useMemo(() => unreadByChat(snapshot), [snapshot]);
  const unreadThreads = useMemo(() => [...unread.entries()].flatMap(([chat, threads]) => threads.map((thread) => ({ chat, thread }))), [unread]);
  /** Opens a thread: its chat shown, the thread brought into view (and so seen). */
  const showThread = (chat: string, thread: string) => {
    setShown({ kind: "chat", id: chat });
    setTarget(thread);
  };

  const status = snapshot.online || snapshot.last_push_at != null ? liveLabel(snapshot.online, snapshot.last_push_at) : { text: "Waiting for the owner's Mac", live: false };
  const chat = shownChat ? snapshot.chats.find((c) => c.id === shownChat) : undefined;
  const note = shown?.kind === "note" ? snapshot.notes.find((n) => n.path === shown.path) : undefined;
  const host = useMemo(() => ({ ...NO_HOST, waitingFor: snapshot.owner.name }), [snapshot.owner.name]);
  const clearTarget = useCallback(() => setTarget(null), []);

  return (
    <ChatHostContext.Provider value={host}>
      <div className="flex h-dvh w-screen flex-col bg-bg text-fg" data-testid="workspace-share-page">
        <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line px-3">
          <img src={icon} alt="Chauffeur" width={18} height={18} className="shrink-0 rounded-[4px]" />
          <span className="min-w-0 truncate text-[13px] font-semibold" title={snapshot.title} data-testid="share-title">
            {snapshot.title}
          </span>
          <span className="hidden text-[11.5px] text-faint sm:inline">by {snapshot.owner.name}</span>
          <span className="flex items-center gap-1.5 text-[11.5px]" title={connected ? status.text : "The page lost the share service: it reconnects on its own"} data-testid="share-live">
            <span className={cx("h-1.5 w-1.5 rounded-full", !connected ? "bg-danger" : status.live ? "bg-success" : "bg-faint")} />
            <span className={status.live ? "text-success" : "text-muted"}>{connected ? status.text : "Reconnecting…"}</span>
          </span>
          {snapshot.role === "viewer" && (
            <Badge tone="neutral" title="You follow the chats; only colleagues who can suggest write to the owner">
              viewer
            </Badge>
          )}
          {unreadThreads.length > 0 && (
            <Button
              size="sm"
              variant="secondary"
              className="shrink-0 text-accent"
              onClick={() => {
                const next = unreadThreads.find((t) => t.thread !== target) ?? unreadThreads[0];
                if (next) showThread(next.chat, next.thread);
              }}
              title="Open the next thread that mentions you"
              data-testid="share-mentions"
            >
              <AtSign size={12} /> {unreadThreads.length === 1 ? "1 mention for you" : `${unreadThreads.length} mentions for you`}
            </Button>
          )}
          <div className="relative ml-auto">
            <Button size="sm" variant={peopleOpen ? "secondary" : "ghost"} onClick={() => setPeopleOpen((on) => !on)} title="Who this workspace is shared with" aria-pressed={peopleOpen} data-testid="share-people-button">
              <Users size={12} /> <span className="hidden sm:inline">Shared with</span>
              <span className="rounded-full bg-accent/15 px-1 text-[10.5px] text-accent">{snapshot.people.length}</span>
            </Button>
            {peopleOpen && <PeoplePanel people={snapshot.people} owner={snapshot.owner.email} viewers={snapshot.viewers} onClose={() => setPeopleOpen(false)} />}
          </div>
          <a className="text-[12px] text-accent" href={`chauffeur://import-workspace?url=${encodeURIComponent(window.location.origin + "/w/" + snapshot.id)}`} title="Creates an independent copy of the conversations and notes; no synchronization" data-testid="import-workspace">Import into Chauffeur</a>
          <div className="flex items-center gap-1" data-testid="share-viewers" title={snapshot.viewers.length === 0 ? "Nobody else is looking" : snapshot.viewers.join(", ")}>
            {snapshot.viewers
              .filter((email) => email !== me.email)
              .slice(0, 6)
              .map((email) => (
                <span key={email} className="inline-flex h-6 w-6 items-center justify-center rounded-full bg-accent/20 text-[10px] font-semibold text-accent" title={email}>
                  {initials(email.split("@")[0] ?? email)}
                </span>
              ))}
            <span className="ml-1 inline-flex h-6 w-6 items-center justify-center rounded-full bg-raised text-[10px] font-semibold text-fg ring-1 ring-line-strong" title={`${me.name} (you)`}>
              {initials(me.name)}
            </span>
            <Button
              size="sm"
              variant="ghost"
              title={`Sign out (${me.email})`}
              onClick={() => void api.signOut().then(onSignedOut, () => setNotice("Could not sign out; try again"))}
              data-testid="share-sign-out"
            >
              <LogOut size={12} />
            </Button>
          </div>
        </div>
        <div className="flex min-h-0 flex-1">
          <nav className="flex w-[240px] shrink-0 flex-col gap-0.5 overflow-y-auto border-r border-line p-2 text-[12.5px]" data-testid="share-sidebar">
            <div className="px-2 pb-1 pt-1 text-[11px] font-medium uppercase tracking-wide text-faint">Chats</div>
            {snapshot.chats.length === 0 && <div className="px-2 text-[12px] text-faint">No chat open.</div>}
            {snapshot.chats.map((c) => (
              <ChatLink key={c.id} chat={c} owner={snapshot.owner.name} active={shownChat === c.id} pending={pendingOf(snapshot.suggestions, c.id)} mentioned={unread.get(c.id)?.length ?? 0} onClick={() => setShown({ kind: "chat", id: c.id })} />
            ))}
            <div className="px-2 pb-1 pt-3 text-[11px] font-medium uppercase tracking-wide text-faint">Notes</div>
            {snapshot.notes.length === 0 && <div className="px-2 text-[12px] text-faint">No notes yet.</div>}
            {snapshot.notes.map((n) => (
              <button
                key={n.path}
                type="button"
                onClick={() => setShown({ kind: "note", path: n.path })}
                className={cx("flex min-w-0 items-center gap-1.5 rounded-md px-2 py-1 text-left", shown?.kind === "note" && shown.path === n.path ? "bg-hover text-fg" : "text-muted hover:bg-hover/60")}
                data-testid="share-note"
              >
                <FileText size={12} className="shrink-0 text-faint" />
                <span className="truncate" title={n.path}>
                  {n.path}
                </span>
              </button>
            ))}
          </nav>
          <main className="flex min-w-0 flex-1 flex-col">
            {chat ? (
              <ChatPane
                key={chat.id}
                chat={chat}
                state={state}
                me={me.email}
                canSuggest={snapshot.role === "suggest"}
                suggestions={snapshot.suggestions.filter((s) => s.chat === chat.id)}
                comments={snapshot.comments.filter((c) => c.chat === chat.id)}
                say={say}
                mentionSetup={mentionSetup}
                unread={unread.get(chat.id)}
                onSeen={markSeen}
                target={target}
                onTargetShown={clearTarget}
              />
            ) : note ? (
              <div className="min-h-0 flex-1 overflow-y-auto" data-testid="share-note-view">
                <div className="mx-auto w-full max-w-[860px] px-6 py-5">
                  <div className="mb-3 text-[11.5px] text-faint">
                    {note.path} · {relativeTime(new Date(note.modified_ms).toISOString())}
                  </div>
                  <Markdown text={note.text} />
                </div>
              </div>
            ) : (
              <div className="flex flex-1 items-center justify-center text-[12.5px] text-faint">Pick a chat or a note.</div>
            )}
          </main>
        </div>
        {notice && (
          <div role="status" className="fixed bottom-4 right-4 z-[60] max-w-[380px] rounded-md border border-line-strong bg-raised px-3 py-2 text-[12.5px] shadow-xl" data-testid="share-notice">
            {notice}
          </div>
        )}
      </div>
    </ChatHostContext.Provider>
  );
}

function pendingOf(suggestions: Suggestion[], chat: string): number {
  return suggestions.filter((s) => s.chat === chat && s.state === "pending").length;
}

function ChatLink({ chat, owner, active, pending, mentioned, onClick }: { chat: SharedChat; owner: string; active: boolean; pending: number; mentioned: number; onClick: () => void }) {
  const status = guestChatStatus(chat.status, owner);
  return (
    <button
      type="button"
      onClick={onClick}
      className={cx("flex min-w-0 items-center gap-1.5 rounded-md px-2 py-1 text-left", active ? "bg-hover text-fg" : "text-muted hover:bg-hover/60")}
      title={status.label}
      data-testid="share-chat"
    >
      {status.tone === "working" ? (
        <Spinner size={10} className="shrink-0 text-working" />
      ) : (
        <span className={cx("h-1.5 w-1.5 shrink-0 rounded-full", status.tone === "attention" ? "bg-attention" : status.tone === "danger" ? "bg-danger" : "bg-faint")} />
      )}
      {chat.kind === "design" && <PenTool size={11} className="shrink-0 text-faint" aria-label="Design chat" />}
      <span className="min-w-0 flex-1 truncate">{chatTitle(chat)}</span>
      {mentioned > 0 && (
        <Badge tone="accent" title={mentioned === 1 ? "A thread mentions you" : `${mentioned} threads mention you`}>
          @{mentioned}
        </Badge>
      )}
      {pending > 0 && (
        <Badge tone="attention" title={`${pending} suggested, waiting for ${owner}`}>
          {pending}
        </Badge>
      )}
    </button>
  );
}

function ChatPane({
  chat,
  state,
  me,
  canSuggest,
  suggestions,
  comments,
  say,
  mentionSetup,
  unread,
  onSeen,
  target,
  onTargetShown,
}: {
  chat: SharedChat;
  state: GuestState;
  me: string;
  /** May suggest, and so comment and reply. */
  canSuggest: boolean;
  suggestions: Suggestion[];
  comments: ShareComment[];
  say: (message: FromGuest) => boolean;
  mentionSetup: MentionSetup;
  /** The threads of this chat that mention the reader unseen. */
  unread: readonly string[] | undefined;
  onSeen: (thread: string) => void;
  target: string | null;
  onTargetShown: () => void;
}) {
  const view = state.views[chat.id];
  const items = useMemo(() => (view ? view.items.filter((item) => !(item.kind === "tool" && item.asked)) : []), [view]);
  const rows = useMemo(() => groupActivity(items), [items]);
  const working = chat.status === "working" || chat.status === "starting" || (view?.working ?? false);
  const [openGroups, setOpenGroups] = useState<ReadonlySet<string>>(() => new Set());
  const activity = useMemo(
    () => ({
      open: openGroups,
      toggle: (key: string) =>
        setOpenGroups((current) => {
          const next = new Set(current);
          if (next.has(key)) next.delete(key);
          else next.add(key);
          return next;
        }),
    }),
    [openGroups],
  );
  const respond = useCallback(async () => {}, []);
  const placed = useMemo(() => placeComments(view?.items ?? [], comments), [view, comments]);
  const unreadSet = useMemo(() => new Set(unread ?? []), [unread]);
  const shareComments = useMemo(
    () => ({
      byItem: placed,
      me,
      onDelete: (comment: ShareComment) => void say({ type: "delete_comment", id: comment.id }),
      onReply: canSuggest
        ? (thread: ShareComment, text: string, mentions: Mention[]) =>
            say({ type: "comment", chat: chat.id, message: "", quote: "", text, reply_to: thread.id, mentions: mentions.length > 0 ? mentions : undefined })
        : null,
      mentionSetup,
      unread: unreadSet,
      onSeen,
      target,
      onTargetShown,
    }),
    [placed, me, say, canSuggest, chat.id, mentionSetup, unreadSet, onSeen, target, onTargetShown],
  );
  // The passage selected, offered for a comment; the one being commented on.
  const [selected, setSelected] = useState<Passage | null>(null);
  const [commenting, setCommenting] = useState<Passage | null>(null);

  // Kept at the end while the guest is there: the conversation grows under them.
  const scroller = useRef<HTMLDivElement>(null);
  const atBottom = useRef(true);
  useEffect(() => {
    const element = scroller.current;
    if (element && atBottom.current) element.scrollTop = element.scrollHeight;
  }, [rows, suggestions.length, comments.length]);

  return (
    <ActivityContext.Provider value={activity}>
      <ShareCommentsContext.Provider value={shareComments}>
        <div
          ref={scroller}
          className="min-h-0 flex-1 overflow-y-auto pb-4 pt-2"
          onScroll={(event) => {
            const element = event.currentTarget;
            atBottom.current = element.scrollTop + element.clientHeight >= element.scrollHeight - 40;
            setSelected(null);
          }}
          onMouseDown={() => setSelected(null)}
          // Read once the browser has settled the selection.
          onMouseUp={() => {
            if (canSuggest) requestAnimationFrame(() => setSelected(selectedPassage(scroller.current)));
          }}
          data-testid="share-chat-view"
        >
          {!view ? (
            <div className="flex h-full items-center justify-center">
              <Spinner size={16} className="text-faint" />
            </div>
          ) : rows.length === 0 ? (
            <div className="flex h-full items-center justify-center text-[12.5px] text-faint">Nothing said in this chat yet.</div>
          ) : (
            rows.map((row, index) => <ChatRowView key={row.key} row={row} last={index === rows.length - 1} working={working} readOnly respond={respond} />)
          )}
          {suggestions.length > 0 && <SuggestionsList suggestions={suggestions} me={me} onDelete={(s) => say({ type: "delete_suggestion", id: s.id })} />}
        </div>
      </ShareCommentsContext.Provider>
      {selected && (
        <CommentSelectionButton
          passage={selected}
          onComment={(passage) => {
            setCommenting(passage);
            setSelected(null);
          }}
        />
      )}
      {canSuggest &&
        (commenting ? (
          <PassageCommentBox
            key={`${commenting.message}:${commenting.quote}`}
            passage={commenting}
            mentionSetup={mentionSetup}
            readers="the owner and the guests"
            onComment={(text, mentions) =>
              say({ type: "comment", chat: chat.id, message: commenting.message, quote: commenting.quote, text, mentions: mentions.length > 0 ? mentions : undefined })
            }
            onClose={() => setCommenting(null)}
          />
        ) : (
          <SuggestBox chat={chat} onSuggest={(text) => say({ type: "suggest", chat: chat.id, text })} />
        ))}
    </ActivityContext.Provider>
  );
}

/** The suggestions made to this chat, every guest's, and what the owner did with each; the guest takes theirs back. */
function SuggestionsList({ suggestions, me, onDelete }: { suggestions: Suggestion[]; me: string; onDelete: (suggestion: Suggestion) => void }) {
  return (
    <div className="mx-auto mt-3 w-full max-w-[860px] px-5" data-testid="share-suggestions">
      <div className="mb-1 text-[11px] font-medium uppercase tracking-wide text-faint">Suggestions</div>
      <div className="flex flex-col gap-1.5">
        {[...suggestions]
          .sort((a, b) => a.at - b.at)
          .map((s) => (
            <div key={s.id} className="rounded-md border border-line bg-panel px-3 py-2" data-testid="share-suggestion" data-state={s.state}>
              <div className="flex items-center gap-2 text-[11.5px] text-muted">
                <span className="font-medium text-fg" title={s.author.email}>
                  {s.author.name}
                </span>
                <span>{relativeTime(new Date(s.at).toISOString())}</span>
                <Badge tone={s.state === "pending" ? "attention" : s.state === "dismissed" ? "neutral" : "success"} className="ml-auto">
                  {suggestionStateLabel(s.state)}
                </Badge>
                {canDeleteSuggestion(s, me) && (
                  <Button size="xs" variant="ghost" onClick={() => onDelete(s)} title="Delete your suggestion: it leaves the owner's cards and this list" data-testid="share-suggestion-delete">
                    <Trash2 size={11} />
                  </Button>
                )}
              </div>
              <div className={cx("mt-1 whitespace-pre-wrap break-words text-[12.5px] selectable", s.state === "dismissed" ? "text-faint line-through" : "text-fg")}>{s.text}</div>
            </div>
          ))}
      </div>
    </div>
  );
}

/** A guest who can suggest writes to the owner here: nothing reaches the agent unless they send it. */
function SuggestBox({ chat, onSuggest }: { chat: SharedChat; onSuggest: (text: string) => boolean }) {
  const [text, setText] = useState("");
  const submit = () => {
    const value = text.trim();
    if (!value || value.length > SUGGESTION_MAX) return;
    if (onSuggest(value)) setText("");
  };
  return (
    <div className="shrink-0 px-5 pb-3 pt-1">
      <div className="mx-auto max-w-[860px] rounded-lg border border-line-strong bg-panel focus-within:border-accent/50">
        <textarea
          value={text}
          onChange={(event) => setText(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
              event.preventDefault();
              submit();
            }
          }}
          rows={3}
          placeholder={`Suggest a message to ${chatTitle(chat)}'s agent: its owner sends it, or not`}
          className="block w-full resize-none bg-transparent px-3 py-2 text-[13px] text-fg outline-none placeholder:text-faint"
          data-testid="share-suggest-input"
        />
        <div className="flex items-center gap-2 px-2 pb-2">
          <span className="text-[11px] text-faint">
            {text.length > SUGGESTION_MAX ? `At most ${SUGGESTION_MAX} characters` : "⌘↵ to suggest · select a passage of the agent's message to comment on it"}
          </span>
          <Button size="sm" variant="primary" className="ml-auto" disabled={!text.trim() || text.length > SUGGESTION_MAX} onClick={submit} data-testid="share-suggest">
            <MessageSquarePlus size={12} /> Suggest
          </Button>
        </div>
      </div>
    </div>
  );
}
