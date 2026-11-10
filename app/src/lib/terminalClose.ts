// Closing a terminal stops its shell, and a closed terminal cannot be reopened: closing one whose
// shell runs a command (a dev server, a build) asks first, whichever way it is closed.

import type { TerminalsActivity } from "./terminalActivity";

/** A close to confirm: the terminals it closes, and the labels of those that run a command. */
export type TerminalClose = { ids: string[]; running: string[] };

/**
 * What closing the terminals `ids` asks first: null when none of them runs a command (they close at
 * once), else the labels `label` gives those that do, in tab order.
 */
export function closeToConfirm<T extends { id: string }>(
  terminals: T[],
  ids: string[],
  activity: TerminalsActivity,
  label: (terminal: T, index: number) => string,
): TerminalClose | null {
  const running = terminals.flatMap((terminal, index) =>
    ids.includes(terminal.id) && activity[terminal.id]?.running ? [label(terminal, index)] : [],
  );
  return running.length > 0 ? { ids, running } : null;
}
