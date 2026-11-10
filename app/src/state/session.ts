// One live subscription per open workspace: replays its history (`after_seq: 0`) with ephemeral
// events, then follows it. Chat and script output go through the pure reducer; terminal output
// goes straight to the terminals' listeners.
//
// A long conversation would make the reader wait for all of it, so a workspace on this machine
// takes it in two: the end of each chat (and the workspace's own events) first, shown at once,
// then what came before, folded apart and put under the end in one go (`partial` until then).

import type { EventEnvelope, ProcessInfo, TerminalInfo } from "@chauffeur/protocol";
import { useEffect, useSyncExternalStore } from "react";
import { base64ToBytes } from "@/lib/format";
import type { Subscription } from "@/lib/rpc";
import {
  commandChanged,
  listedTerminals,
  terminalExited,
  terminalGone,
  terminalOpened,
  terminalSeen,
  type TerminalsActivity,
} from "@/lib/terminalActivity";
import { type ChatView, emptyChatView, reduceChat } from "@/lib/chat";
import {
  emptyWorkspaceLive,
  mergeProcessList,
  reduceWorkspace,
  type WorkspaceLive,
} from "@/lib/workspaceLive";
import { client, forgetFileIndex, reviewerCommented, useApp } from "./app";

/** Events of each chat that come first: a few screens of a conversation, tool calls included. */
const TAIL_PER_CHAT = 300;
/** How long the older events are folded at a time before the page may answer a click. */
const SLICE_MS = 10;

type TerminalListener = {
  /** A live chunk; `offset` is its first byte's offset in the terminal's whole output. */
  output: (bytes: Uint8Array, offset: number) => void;
  exit: (code: number | null) => void;
};

export class WorkspaceSession {
  state: WorkspaceLive = emptyWorkspaceLive;
  private listeners = new Set<() => void>();
  private terminals = new Map<string, Set<TerminalListener>>();
  private subscription!: Subscription;
  /** What came before the tail, while it is read: folded apart, never shown as it is. */
  private head: Record<string, ChatView> | null = null;
  private headSubscription: Subscription | null = null;
  /** Older events received and not folded yet, the daemon's `replayed` last. */
  private headQueue: EventEnvelope[] = [];
  private headTimer: ReturnType<typeof setTimeout> | null = null;
  /** The chat events seen meanwhile, to apply on top of the head. */
  private held: EventEnvelope[] = [];
  /** Where the workspace runs, when the subscription opened: another machine numbers its events. */
  movedTo: string | null;
  private scheduled = false;
  /** The terminal on screen: a command finishing there is seen at once. */
  private viewedTerminal: string | null = null;
  refs = 0;

  constructor(readonly workspaceId: string) {
    this.movedTo = useApp.getState().workspaces[workspaceId]?.moved_to ?? null;
    this.follow();
    this.relistTerminals();
    window.addEventListener("focus", this.relistTerminals);
  }

  /**
   * Activity events are ephemeral, so what runs already comes from the list; and no event says
   * that a terminal opened or closed in another client of the daemon: listed again on focus.
   */
  private relistTerminals = () => {
    client
      .call("terminal.list", { workspace: this.workspaceId })
      .then((result) => this.listTerminals(result.terminals))
      .catch(() => {});
  };

  /** Replays the workspace's history from the start, then follows it. */
  private follow() {
    const workspaceId = this.workspaceId;
    const filter = { all: false, registry: false, workspaces: [workspaceId], chats: [], ephemeral: true, after_seq: 0 };
    this.subscription = client.subscribe(
      { ...filter, replay: { part: "tail", per_chat: TAIL_PER_CHAT } },
      (envelope) => this.onEnvelope(envelope),
    );
    this.state = { ...this.state, partial: true };
    this.head = {};
    this.held = [];
  }

  private readHead() {
    if (!this.head || this.headSubscription) return;
    // Start after the tail has arrived. A later cutoff can overlap it (deduplicated on merge);
    // starting both requests at once could leave a gap when new events move the tail forward.
    this.headSubscription = client.subscribe(
      { all: false, registry: false, workspaces: [this.workspaceId], chats: [], ephemeral: true, after_seq: 0,
        replay: { part: "before_tail", per_chat: TAIL_PER_CHAT } },
      (envelope) => this.onHead(envelope),
    );
  }

  private stopHead() {
    this.headSubscription?.close();
    this.headSubscription = null;
    if (this.headTimer) clearTimeout(this.headTimer);
    this.headTimer = null;
    this.headQueue = [];
    this.head = null;
    this.held = [];
  }

  /** What came before the tail: queued, then folded a slice at a time so that the page stays alive. */
  private onHead(envelope: EventEnvelope) {
    if (!this.head) return;
    this.headQueue.push(envelope);
    this.headTimer ??= setTimeout(() => this.foldHead(), 0);
  }

  private foldHead() {
    this.headTimer = null;
    const started = performance.now();
    while (this.head && this.headQueue.length > 0) {
      if (performance.now() - started > SLICE_MS) {
        this.headTimer = setTimeout(() => this.foldHead(), 0);
        return;
      }
      const envelope = this.headQueue.shift()!;
      const event = envelope.event;
      if (event.scope === "workspace" && event.event.type === "replayed") {
        this.mergeHead(this.head);
        return;
      }
      // Only the chats' persisted events count; each chat's view skips what it has seen.
      if (event.scope === "chat" && envelope.seq != null) {
        this.head[event.chat_id] = reduceChat(this.head[event.chat_id] ?? emptyChatView, envelope);
      }
    }
  }

  /** The end, as it grew meanwhile, goes on top of what came before it. Chat by chat: their numbers interleave. */
  private mergeHead(head: Record<string, ChatView>) {
    const chats = { ...head };
    for (const envelope of this.held) {
      const event = envelope.event;
      if (event.scope === "chat") chats[event.chat_id] = reduceChat(chats[event.chat_id] ?? emptyChatView, envelope);
    }
    this.stopHead();
    this.state = { ...this.state, chats, partial: false };
    this.schedule();
  }

  /**
   * The workspace moved, or came back: its events are now the other machine's, numbered there.
   * The history is read again from the start, the terminals' activity kept.
   */
  moved(movedTo: string | null) {
    if (movedTo === this.movedTo) return;
    this.movedTo = movedTo;
    this.subscription.close();
    this.stopHead();
    this.state = { ...emptyWorkspaceLive, terminals: this.state.terminals };
    this.follow();
    this.schedule();
  }

  private onEnvelope(envelope: EventEnvelope) {
    const event = envelope.event;
    if (event.scope === "chat" && event.event.type === "design_share") {
      reviewerCommented(this.workspaceId, event.event.state);
      return;
    }
    if (event.scope === "workspace" && event.event.type === "file_share") {
      reviewerCommented(this.workspaceId, event.event.state);
      return;
    }
    if (event.scope === "workspace") {
      const e = event.event;
      if (e.type === "replayed") this.readHead();
      if (e.type === "terminal_output") {
        const listeners = this.terminals.get(e.terminal_id);
        if (listeners?.size) {
          const bytes = base64ToBytes(e.data);
          for (const listener of listeners) listener.output(bytes, e.offset);
        }
        return;
      }
      if (e.type === "terminal_activity") {
        const viewed = e.terminal_id === this.viewedTerminal;
        this.setTerminals(commandChanged(this.state.terminals, e.terminal_id, e.command_running, viewed));
        return;
      }
      if (e.type === "terminal_exited") {
        for (const listener of this.terminals.get(e.terminal_id) ?? []) listener.exit(e.exit_code);
        this.setTerminals(terminalExited(this.state.terminals, e.terminal_id));
      }
    }
    if (this.head && event.scope === "chat") this.held.push(envelope);
    const next = reduceWorkspace(this.state, envelope);
    if (next !== this.state) {
      this.state = next;
      this.schedule();
    }
  }

  /** Coalesces a replay's thousands of events into one render per frame. */
  private schedule() {
    if (this.scheduled) return;
    this.scheduled = true;
    const flush = () => {
      this.scheduled = false;
      for (const listener of this.listeners) listener();
    };
    if (typeof requestAnimationFrame === "function" && document.visibilityState === "visible") {
      requestAnimationFrame(flush);
    } else {
      setTimeout(flush, 16);
    }
  }

  mergeProcesses(processes: ProcessInfo[]) {
    this.state = mergeProcessList(this.state, processes);
    this.schedule();
  }

  listTerminals(terminals: TerminalInfo[]) {
    this.setTerminals(listedTerminals(this.state.terminals, terminals));
  }

  terminalOpened(terminalId: string) {
    this.setTerminals(terminalOpened(this.state.terminals, terminalId));
  }

  terminalClosed(terminalId: string) {
    this.setTerminals(terminalGone(this.state.terminals, terminalId));
  }

  /** The terminal now on screen (none: the Terminal view hidden), its finished command seen. */
  viewTerminal(terminalId: string | null) {
    this.viewedTerminal = terminalId;
    if (terminalId) this.setTerminals(terminalSeen(this.state.terminals, terminalId));
  }

  private setTerminals(terminals: TerminalsActivity) {
    if (terminals === this.state.terminals) return;
    this.state = { ...this.state, terminals };
    this.schedule();
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  getSnapshot = () => this.state;

  onTerminal(terminalId: string, listener: TerminalListener): () => void {
    let set = this.terminals.get(terminalId);
    if (!set) {
      set = new Set();
      this.terminals.set(terminalId, set);
    }
    set.add(listener);
    return () => {
      set.delete(listener);
    };
  }

  close() {
    window.removeEventListener("focus", this.relistTerminals);
    this.subscription.close();
    this.stopHead();
    this.listeners.clear();
    this.terminals.clear();
  }
}

const sessions = new Map<string, WorkspaceSession>();
// A workspace that moves or comes back is followed where it runs now.
useApp.subscribe((state) => {
  for (const [id, session] of sessions) {
    const workspace = state.workspaces[id];
    if (workspace) session.moved(workspace.moved_to ?? null);
  }
});
/** Recently viewed workspaces keep their session so switching back is instant. */
const recent: string[] = [];
const KEEP = 6;

function acquire(workspaceId: string): WorkspaceSession {
  let session = sessions.get(workspaceId);
  if (!session) {
    session = new WorkspaceSession(workspaceId);
    sessions.set(workspaceId, session);
  }
  session.refs += 1;
  const index = recent.indexOf(workspaceId);
  if (index >= 0) recent.splice(index, 1);
  recent.push(workspaceId);
  return session;
}

function release(session: WorkspaceSession) {
  session.refs -= 1;
  while (recent.length > KEEP) {
    const id = recent[0];
    const candidate = sessions.get(id);
    if (candidate && candidate.refs > 0) break;
    recent.shift();
    candidate?.close();
    sessions.delete(id);
    forgetFileIndex(id);
  }
}

export function getSession(workspaceId: string): WorkspaceSession | undefined {
  return sessions.get(workspaceId);
}

/** The live session of a workspace, held while the component is mounted. */
export function useWorkspaceSession(workspaceId: string): WorkspaceSession {
  const session = sessions.get(workspaceId) ?? acquireDetached(workspaceId);
  useEffect(() => {
    const held = acquire(workspaceId);
    return () => release(held);
  }, [workspaceId]);
  return session;
}

/** Creates the session during render (before the effect) without taking a reference. */
function acquireDetached(workspaceId: string): WorkspaceSession {
  const session = new WorkspaceSession(workspaceId);
  sessions.set(workspaceId, session);
  return session;
}

export function useWorkspaceLive(session: WorkspaceSession): WorkspaceLive {
  return useSyncExternalStore(session.subscribe, session.getSnapshot);
}

/**
 * One part of the live state; the component re-renders only when it changes. The selector must
 * return a part of the state as is (e.g. `(s) => s.log`), not a new object.
 */
export function useWorkspaceSelector<T>(session: WorkspaceSession, selector: (state: WorkspaceLive) => T): T {
  return useSyncExternalStore(session.subscribe, () => selector(session.state));
}
