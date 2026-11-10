// Event stream of one workspace → its chats, its log (progress, warnings, state changes) and its
// scripts' output. Pure; terminal output is not kept here (it goes straight to xterm.js), and the
// terminals' activity is the session's (it depends on the terminal on screen).

import type {
  EventEnvelope,
  OutputStream,
  ProcessInfo,
  ProcessKind,
  TransferInfo,
  WorkspaceState,
} from "@chauffeur/protocol";
import { type ChatView, emptyChatView, reduceChat } from "./chat";
import type { TerminalsActivity } from "./terminalActivity";

export type LogEntry = {
  key: string;
  seq: number | null;
  ts: string;
  level: "progress" | "warning" | "state" | "error";
  message: string;
};

export type ProcessLine = { stream: OutputStream; line: string };

/**
 * A process's last lines: `buffer[start..end)`. States that follow one another share the buffer,
 * so that a line is added without copying the others; a state never sees the lines added after it.
 * Read it with `processLines`.
 */
export type ProcessOutput = { readonly buffer: ProcessLine[]; readonly start: number; readonly end: number };

export type ProcessView = {
  id: string;
  kind: ProcessKind;
  repo: string | null;
  name: string;
  command: string;
  running: boolean;
  exitCode: number | null;
  output: ProcessOutput;
};

export type WorkspaceLive = {
  chats: Record<string, ChatView>;
  log: LogEntry[];
  processes: Record<string, ProcessView>;
  /** Order processes were first seen in. */
  processOrder: string[];
  /** Bumped on every `turn_ended`: views that depend on the files refresh on it. */
  turnsEnded: number;
  /** Bumped when worktrees are added or removed. */
  worktreesChanged: number;
  /** Set by the session from `terminal.list` and `terminal_activity`. */
  terminals: TerminalsActivity;
  lastSeq: number;
  /** The daemon has sent the whole history (`replayed`): what comes next is live. */
  live: boolean;
  /**
   * Only the end of each chat is here: the rest is still coming (set by the session). What counts
   * the whole conversation (its turns, its cost) or looks through it is not right until it is not.
   */
  partial: boolean;
  /** The workspace's transfer as its last event told, while the view is open. */
  transfer: TransferInfo | null;
  /** Moved to another machine: whether this daemon reaches it, as its last event told. */
  relay: RelayStatus | null;
};

export type RelayStatus = { machine: string; connected: boolean; error: string | null };

export const MAX_PROCESS_LINES = 2000;

const NO_OUTPUT: ProcessOutput = { buffer: [], start: 0, end: 0 };

function outputOf(lines: ProcessLine[]): ProcessOutput {
  const kept = lines.slice(-MAX_PROCESS_LINES);
  return { buffer: kept, start: 0, end: kept.length };
}

/** The lines of a process's output, oldest first, at most `MAX_PROCESS_LINES`. */
export function processLines({ buffer, start, end }: ProcessOutput): ProcessLine[] {
  return buffer.slice(start, end);
}

/**
 * Adds a line in amortised constant time: pushed onto the shared buffer when this state's lines
 * end it, else onto a copy (an older state branched off). The buffer is cut back once it holds
 * twice the lines kept.
 */
export function appendLine(output: ProcessOutput, line: ProcessLine): ProcessOutput {
  let { buffer, start, end } = output;
  if (buffer.length !== end) {
    buffer = buffer.slice(start, end);
    end -= start;
    start = 0;
  }
  buffer.push(line);
  end += 1;
  start = Math.max(start, end - MAX_PROCESS_LINES);
  if (start >= MAX_PROCESS_LINES) {
    buffer = buffer.slice(start, end);
    end -= start;
    start = 0;
  }
  return { buffer, start, end };
}
const MAX_LOG = 500;

export const emptyWorkspaceLive: WorkspaceLive = {
  chats: {},
  log: [],
  processes: {},
  processOrder: [],
  turnsEnded: 0,
  worktreesChanged: 0,
  terminals: {},
  lastSeq: 0,
  live: false,
  partial: false,
  transfer: null,
  relay: null,
};

export function reduceWorkspace(state: WorkspaceLive, envelope: EventEnvelope): WorkspaceLive {
  if (envelope.seq != null && envelope.seq <= state.lastSeq) return state;
  const next = apply(state, envelope);
  if (envelope.seq != null) return { ...next, lastSeq: envelope.seq };
  return next;
}

function apply(state: WorkspaceLive, envelope: EventEnvelope): WorkspaceLive {
  const event = envelope.event;
  if (event.scope === "chat") {
    const before = state.chats[event.chat_id] ?? emptyChatView;
    const after = reduceChat(before, envelope);
    const chats = after === before ? state.chats : { ...state.chats, [event.chat_id]: after };
    const turnsEnded = state.turnsEnded + (event.event.type === "turn_ended" ? 1 : 0);
    if (chats === state.chats && turnsEnded === state.turnsEnded) return state;
    return { ...state, chats, turnsEnded };
  }
  if (event.scope !== "workspace") return state;
  const e = event.event;
  const log = (level: LogEntry["level"], message: string): WorkspaceLive => ({
    ...state,
    log: [
      ...state.log.slice(-(MAX_LOG - 1)),
      { key: `${envelope.seq ?? envelope.ts}-${state.log.length}`, seq: envelope.seq, ts: envelope.ts, level, message },
    ],
  });
  switch (e.type) {
    case "progress":
      return log("progress", e.message);
    case "warning":
      return log("warning", e.message);
    case "state_changed":
      return log(e.error ? "error" : "state", stateMessage(e.state, e.error));
    case "transfer":
      return { ...state, transfer: e.transfer };
    case "replayed":
      return state.live ? state : { ...state, live: true };
    case "relay":
      return { ...state, relay: { machine: e.machine, connected: e.connected, error: e.error } };
    case "worktree_added":
    case "worktree_removed":
      return { ...state, worktreesChanged: state.worktreesChanged + 1 };
    case "process_started": {
      const existing = state.processes[e.process_id];
      const process: ProcessView = {
        id: e.process_id,
        kind: e.kind,
        repo: e.repo,
        name: e.name,
        command: e.command,
        running: true,
        exitCode: null,
        output: existing?.output ?? NO_OUTPUT,
      };
      return {
        ...state,
        processes: { ...state.processes, [e.process_id]: process },
        processOrder: existing ? state.processOrder : [...state.processOrder, e.process_id],
      };
    }
    case "process_output": {
      const process = state.processes[e.process_id];
      if (!process) {
        // Output of a process started before the replay window: keep it under a placeholder.
        const placeholder: ProcessView = {
          id: e.process_id,
          kind: "run",
          repo: null,
          name: e.process_id.slice(0, 8),
          command: "",
          running: true,
          exitCode: null,
          output: outputOf([{ stream: e.stream, line: e.line }]),
        };
        return {
          ...state,
          processes: { ...state.processes, [e.process_id]: placeholder },
          processOrder: [...state.processOrder, e.process_id],
        };
      }
      const output = appendLine(process.output, { stream: e.stream, line: e.line });
      return { ...state, processes: { ...state.processes, [e.process_id]: { ...process, output } } };
    }
    case "process_exited": {
      const process = state.processes[e.process_id];
      if (!process) return state;
      return {
        ...state,
        processes: {
          ...state.processes,
          [e.process_id]: { ...process, running: false, exitCode: e.exit_code },
        },
      };
    }
    case "terminal_output":
    case "terminal_exited":
    case "terminal_activity":
    case "share":
    case "file_share":
      return state;
  }
}

function stateMessage(state: WorkspaceState, error: string | null): string {
  const label = state.replace("_", " ");
  return error ? `${label}: ${error}` : label;
}

/**
 * Merges `process.list` into the live view: the daemon's tail fills processes whose output was
 * not seen live (output lines are ephemeral, never replayed).
 */
export function mergeProcessList(state: WorkspaceLive, processes: ProcessInfo[]): WorkspaceLive {
  let changed = false;
  const merged = { ...state.processes };
  const order = [...state.processOrder];
  for (const info of processes) {
    const existing = merged[info.id];
    const output =
      existing && existing.output.end - existing.output.start >= info.tail.length
        ? existing.output
        : outputOf(info.tail.map((line) => ({ stream: "stdout" as const, line })));
    merged[info.id] = {
      id: info.id,
      kind: info.kind,
      repo: info.repo,
      name: info.name,
      command: info.command,
      running: info.running,
      exitCode: info.exit_code,
      output,
    };
    if (!existing) order.push(info.id);
    changed = true;
  }
  if (!changed) return state;
  return { ...state, processes: merged, processOrder: order };
}
