// Whether each terminal of a workspace runs a command, and which finished one the user has not
// seen yet: from `terminal.list` and the ephemeral `terminal_activity` events. Kept for this run of
// the app only: a command that finished while the app was closed is not marked.

import type { TerminalInfo } from "@chauffeur/protocol";

export type TerminalActivity = {
  /** A command runs in the terminal's foreground. */
  running: boolean;
  /** A command finished while the user was not looking at the terminal. */
  unseen: boolean;
};

export type TerminalsActivity = Record<string, TerminalActivity>;

/** A command started or finished; one that finishes in the terminal on screen is seen at once. */
export function commandChanged(
  state: TerminalsActivity,
  terminalId: string,
  running: boolean,
  viewed: boolean,
): TerminalsActivity {
  const unseen = viewed ? false : !running || (state[terminalId]?.unseen ?? false);
  return { ...state, [terminalId]: { running, unseen } };
}

/** The daemon's list: where each terminal stands, the unseen marks kept, closed terminals gone. */
export function listedTerminals(state: TerminalsActivity, terminals: TerminalInfo[]): TerminalsActivity {
  return Object.fromEntries(
    terminals.map((t) => [t.id, { running: t.running && t.command_running, unseen: state[t.id]?.unseen ?? false }]),
  );
}

export function terminalSeen(state: TerminalsActivity, terminalId: string): TerminalsActivity {
  const activity = state[terminalId];
  if (!activity?.unseen) return state;
  return { ...state, [terminalId]: { ...activity, unseen: false } };
}

/** A terminal opened from the app: counted at once, before any command runs in it. */
export function terminalOpened(state: TerminalsActivity, terminalId: string): TerminalsActivity {
  if (terminalId in state) return state;
  return { ...state, [terminalId]: { running: false, unseen: false } };
}

/** Its shell exited: the terminal stays open, as its tab does, until it is closed. */
export function terminalExited(state: TerminalsActivity, terminalId: string): TerminalsActivity {
  const activity = state[terminalId];
  if (!activity?.running) return state;
  return { ...state, [terminalId]: { ...activity, running: false } };
}

export function terminalGone(state: TerminalsActivity, terminalId: string): TerminalsActivity {
  if (!(terminalId in state)) return state;
  const next = { ...state };
  delete next[terminalId];
  return next;
}

/** What the workspace's Terminal tab shows: a finished command not seen, else one running. */
export function terminalsIndicator(state: TerminalsActivity): "unseen" | "running" | null {
  const all = Object.values(state);
  if (all.some((a) => a.unseen)) return "unseen";
  return all.some((a) => a.running) ? "running" : null;
}
