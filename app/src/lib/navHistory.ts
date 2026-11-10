// The workspaces the user went through, as a browser keeps its pages: a stack behind the one
// shown and a stack ahead of it. Kept in memory only; it does not know the Office.

export type NavHistory = { back: string[]; forward: string[] };

export const EMPTY_NAV: NavHistory = { back: [], forward: [] };

const MAX_STEPS = 50;

/** The history after the user left `from` for `to`: what was ahead is gone. */
export function visit(history: NavHistory, from: string | null, to: string): NavHistory {
  if (from === to) return history;
  const back = from === null ? history.back : [...history.back, from].slice(-MAX_STEPS);
  return { back, forward: [] };
}

/** Whether a workspace could be reached by `step`. */
type Reachable = (id: string) => boolean;

/** Whether there is a reachable workspace behind (-1) or ahead (1). */
export function canStep(history: NavHistory, direction: 1 | -1, reachable: Reachable): boolean {
  return (direction < 0 ? history.back : history.forward).some(reachable);
}

/**
 * The workspace behind (-1) or ahead (1) of `current` and the history once it is shown; the ones
 * no longer reachable (archived, gone) are passed over. Null when there is none.
 */
export function step(
  history: NavHistory,
  current: string | null,
  direction: 1 | -1,
  reachable: Reachable,
): { id: string; history: NavHistory } | null {
  const [source, other] = direction < 0 ? [history.back, history.forward] : [history.forward, history.back];
  const index = source.findLastIndex(reachable);
  if (index < 0) return null;
  const rest = source.slice(0, index);
  const left = current === null ? other : [...other, current].slice(-MAX_STEPS);
  return {
    id: source[index],
    history: direction < 0 ? { back: rest, forward: left } : { back: left, forward: rest },
  };
}
