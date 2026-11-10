// The sidebar's multiple selection, as in Finder: ⌘-click adds or removes a workspace, ⇧-click
// adds the rows from the last one picked to the one clicked. The first pick keeps the workspace
// open beside it.

/** ⌘-click on `id`: added, or removed when picked already; the open workspace is picked with the
 * first one. */
export function togglePicked(picked: string[], id: string, open: string | null): string[] {
  if (picked.includes(id)) return picked.filter((p) => p !== id);
  const base = picked.length === 0 && open && open !== id ? [open] : picked;
  return [...base, id];
}

/** ⇧-click on `id`: the rows from `anchor` (the last picked, else the open workspace) to `id`, in
 * the sidebar's order, added to the selection; just `id` when the anchor is not shown. */
export function pickRange(picked: string[], order: string[], anchor: string | null, id: string): string[] {
  const from = anchor ? order.indexOf(anchor) : -1;
  const to = order.indexOf(id);
  if (to < 0) return picked;
  const range = from < 0 ? [id] : order.slice(Math.min(from, to), Math.max(from, to) + 1);
  return [...picked, ...range.filter((r) => !picked.includes(r))];
}
