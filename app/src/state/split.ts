// The workspace view's split: actions on the layout kept per workspace in the app store, and
// written to localStorage so that it comes back after a restart of the app.

import {
  closeSide,
  LAYOUTS_KEY,
  moveToMain,
  openSide,
  serializeLayouts,
  type SplitLayout,
  type SplitPosition,
  toggleSide,
  type ViewId,
} from "@/lib/splitLayout";
import { availableLayout, viewAvailable } from "@/lib/extensions";
import { mainView, splitOf, useApp } from "./app";

const set = useApp.setState;
const get = useApp.getState;

function save(workspaceId: string) {
  try {
    localStorage.setItem(LAYOUTS_KEY, serializeLayouts(get().split, workspaceId));
  } catch {
    // Storage full or unavailable: the layout holds until the app restarts.
  }
}

function change(workspaceId: string, next: (layout: SplitLayout, main: ViewId) => { layout: SplitLayout; main: ViewId }) {
  set((s) => {
    const { layout, main } = next(splitOf(s, workspaceId), mainView(s, workspaceId));
    return { split: { ...s.split, [workspaceId]: availableLayout(s.extensions, layout) }, tab: { ...s.tab, [workspaceId]: viewAvailable(s.extensions, main) ? main : "chat" } };
  });
  save(workspaceId);
}

/** Puts a view in the side pane, on the right or at the bottom. */
export function openOnSide(workspaceId: string, view: ViewId, position: SplitPosition) {
  if (!viewAvailable(get().extensions, view)) return;
  change(workspaceId, (layout, main) => openSide(layout, main, view, position));
}

/** Closes the side pane; ⌘\ opens it again. */
export function closeSplit(workspaceId: string) {
  change(workspaceId, (layout, main) => ({ layout: closeSide(layout), main }));
}

/** ⌘\: closes the side pane, or opens its last view (Terminal on the right at first). Whether it opened. */
export function toggleSplit(workspaceId: string): boolean {
  change(workspaceId, toggleSide);
  return splitOf(get(), workspaceId).side != null;
}

/** Moves the side pane to the right or to the bottom. */
export function placeSplit(workspaceId: string, position: SplitPosition) {
  change(workspaceId, (layout, main) => ({ layout: { ...layout, position }, main }));
}

/** Shows a view in the main pane, out of the side pane if it was there. */
export function moveToMainArea(workspaceId: string, view: ViewId) {
  if (!viewAvailable(get().extensions, view)) return;
  change(workspaceId, (layout) => moveToMain(layout, view));
}

/** The side pane's share of the view area; kept once `done` (the divider let go). */
export function resizeSplit(workspaceId: string, share: number, done: boolean) {
  set((s) => {
    const layout = splitOf(s, workspaceId);
    return { split: { ...s.split, [workspaceId]: { ...layout, size: { ...layout.size, [layout.position]: share } } } };
  });
  if (done) save(workspaceId);
}

/** Whether the view area is too small for the split right now. */
export function setSplitNarrow(workspaceId: string, narrow: boolean) {
  if ((get().splitNarrow[workspaceId] ?? false) === narrow) return;
  set((s) => ({ splitNarrow: { ...s.splitNarrow, [workspaceId]: narrow } }));
}
