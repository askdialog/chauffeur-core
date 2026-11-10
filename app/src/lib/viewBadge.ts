// What a view's tab (Chat, Terminal, Processes) shows after its label: that something is open in
// it, and how many once there are several. The count stays faint; an activity mark (a command
// running or finished in a terminal, an agent working or waiting for the user) keeps its colour
// and takes the place of the faint dot, beside the count.

export type ViewActivity = "running" | "unseen" | "working" | "attention";

export type ViewBadge = {
  /** Shown when more than one thing is open. */
  count: number | null;
  dot: "faint" | ViewActivity | null;
};

export function viewBadge(open: number, activity: ViewActivity | null = null): ViewBadge {
  return { count: open > 1 ? open : null, dot: activity ?? (open === 1 ? "faint" : null) };
}
