// The order of the sidebar's workspaces: pinned first, then the user's sections in their order,
// then the others ("Workspaces"); where a dragged workspace goes; where the selection goes when
// one leaves the sidebar.

import type { SectionInfo, WorkspaceInfo } from "@chauffeur/protocol";

export type SidebarRow = { workspace: WorkspaceInfo; depth: number };

/** A group of the sidebar, under its heading. */
export type SidebarGroup =
  | { kind: "pinned"; key: "pinned"; rows: SidebarRow[] }
  | { kind: "section"; key: string; section: SectionInfo; rows: SidebarRow[] }
  | { kind: "workspaces"; key: "workspaces"; rows: SidebarRow[] };

/** The section a workspace shows in, when it is not pinned: null for one that no longer exists. */
function sectionOf(workspace: WorkspaceInfo, known: Set<string>): string | null {
  return workspace.section_id && known.has(workspace.section_id) ? workspace.section_id : null;
}

export function orderedWorkspaces(
  workspaces: Record<string, WorkspaceInfo>,
  archived: boolean,
  sections: SectionInfo[] = [],
): SidebarRow[] {
  // A workspace being archived is already out of the sidebar; it comes back if archiving fails.
  const isArchived = (w: WorkspaceInfo) => w.state === "archived" || w.state === "archiving";
  const all = Object.values(workspaces).filter((w) => isArchived(w) === archived);
  if (archived) {
    return all
      .sort((a, b) => (b.archived_at ?? "").localeCompare(a.archived_at ?? "") || b.created_at.localeCompare(a.created_at) || a.id.localeCompare(b.id))
      .map((workspace) => ({ workspace, depth: 0 }));
  }
  // The user's order (`position`, highest first; newest first until moved), the newest first among equals.
  const byNewest = (a: WorkspaceInfo, b: WorkspaceInfo) =>
    (b.position ?? 0) - (a.position ?? 0) || b.created_at.localeCompare(a.created_at);
  const rank = new Map(sections.map((s, i) => [s.id, i]));
  const known = new Set(rank.keys());
  const groupRank = (w: WorkspaceInfo) => {
    if (w.pinned) return -1;
    const section = sectionOf(w, known);
    return section == null ? sections.length : rank.get(section)!;
  };
  const ids = new Set(all.map((w) => w.id));
  // Pinned first, then each section, then the others, each part in the user's order.
  const roots = all
    .filter((w) => !w.parent_id || !ids.has(w.parent_id))
    .sort((a, b) => groupRank(a) - groupRank(b) || byNewest(a, b));
  const out: SidebarRow[] = [];
  for (const root of roots) {
    out.push({ workspace: root, depth: root.parent_id ? 1 : 0 });
    const children = all
      .filter((w) => w.parent_id === root.id)
      .sort((a, b) => a.created_at.localeCompare(b.created_at));
    for (const child of children) out.push({ workspace: child, depth: 1 });
  }
  return out;
}

/** The key of the group a top-level workspace shows in. */
function groupKey(workspace: WorkspaceInfo, known: Set<string>): string {
  if (workspace.pinned) return "pinned";
  return sectionOf(workspace, known) ?? "workspaces";
}

/**
 * The sidebar's groups, in order: Pinned (only when a workspace is pinned), every section (even
 * empty, to drop workspaces in), then Workspaces. Children come under their parent, in its group.
 */
export function sidebarGroups(workspaces: Record<string, WorkspaceInfo>, sections: SectionInfo[]): SidebarGroup[] {
  const known = new Set(sections.map((s) => s.id));
  const rows = orderedWorkspaces(workspaces, false, sections);
  const byKey = new Map<string, SidebarRow[]>();
  let key = "workspaces";
  for (const row of rows) {
    // A child follows its parent's row; a top-level row starts its group's run.
    if (row.depth === 0 || !rows.some((r) => r.workspace.id === row.workspace.parent_id)) key = groupKey(row.workspace, known);
    byKey.set(key, [...(byKey.get(key) ?? []), row]);
  }
  const groups: SidebarGroup[] = [];
  const pinned = byKey.get("pinned") ?? [];
  if (pinned.length > 0) groups.push({ kind: "pinned", key: "pinned", rows: pinned });
  for (const section of sections) groups.push({ kind: "section", key: section.id, section, rows: byKey.get(section.id) ?? [] });
  groups.push({ kind: "workspaces", key: "workspaces", rows: byKey.get("workspaces") ?? [] });
  return groups;
}

/** The rows of a group that are shown: the children of a collapsed parent are left out. */
export function shownRows(rows: SidebarRow[], collapsed: ReadonlySet<string>): SidebarRow[] {
  return rows.filter((r) => !(r.depth > 0 && r.workspace.parent_id && collapsed.has(r.workspace.parent_id)));
}

/** The rows shown, in order: what ⌘1–9 select. A folded section's rows and a collapsed parent's children are skipped. */
export function visibleRows(groups: SidebarGroup[], collapsed: ReadonlySet<string> = new Set()): SidebarRow[] {
  return groups.flatMap((g) => (g.kind === "section" && g.section.folded ? [] : shownRows(g.rows, collapsed)));
}

/** What a folded section still tells: whether one of its workspaces works, or needs the user. */
export function foldedIndicator(rows: SidebarRow[]): { working: boolean; needsInput: boolean } {
  const ready = rows.map((r) => r.workspace).filter((w) => w.state === "ready");
  return {
    working: ready.some((w) => w.activity === "working"),
    needsInput: ready.some((w) => w.activity === "needs_input"),
  };
}

/**
 * The workspace to show instead of `id`, which leaves the list: the next one, else the one above,
 * else none; its children leave with it. Undefined when `id` was not in the list (already
 * archiving, opened from the switcher): the user chose to look at it.
 */
export function neighbourOf(
  workspaces: Record<string, WorkspaceInfo>,
  id: string,
  sections: SectionInfo[] = [],
): string | null | undefined {
  const list = orderedWorkspaces(workspaces, false, sections)
    .map((row) => row.workspace)
    .filter((w) => w.id === id || w.parent_id !== id);
  const at = list.findIndex((w) => w.id === id);
  if (at === -1) return undefined;
  return list[at + 1]?.id ?? list[at - 1]?.id ?? null;
}

/** The workspaces that move in the sidebar, in its order: those whose parent is not shown. */
export function topLevel(workspaces: Record<string, WorkspaceInfo>, sections: SectionInfo[] = []): WorkspaceInfo[] {
  const rows = orderedWorkspaces(workspaces, false, sections).map((row) => row.workspace);
  const shown = new Set(rows.map((w) => w.id));
  return rows.filter((w) => !w.parent_id || !shown.has(w.parent_id));
}

/** Where a workspace goes: above `before` (last when null), pinned or in `section` (null: none). */
export type DropPlace = { before: string | null; pinned: boolean; section: string | null };

/**
 * Where a workspace dropped on `target`'s row goes: above it, or below it (above the next
 * top-level workspace of the same group, else last), in the target's group (pinned, a section or
 * the others). A child's row stands for the bottom of its parent's block.
 */
export function dropPlace(
  workspaces: Record<string, WorkspaceInfo>,
  target: string,
  below: boolean,
  sections: SectionInfo[] = [],
): DropPlace | null {
  const known = new Set(sections.map((s) => s.id));
  const tops = topLevel(workspaces, sections);
  let at = tops.findIndex((w) => w.id === target);
  if (at === -1) {
    at = tops.findIndex((w) => w.id === workspaces[target]?.parent_id);
    below = true;
  }
  if (at === -1) return null;
  const group = groupKey(tops[at], known);
  const place = (before: string | null): DropPlace =>
    group === "pinned"
      ? { before, pinned: true, section: null }
      : { before, pinned: false, section: group === "workspaces" ? null : group };
  if (!below) return place(tops[at].id);
  const next = tops[at + 1];
  return place(next && groupKey(next, known) === group ? next.id : null);
}

/** Where a workspace dropped on a group's heading goes: on top of that group. */
export function headingDropPlace(
  workspaces: Record<string, WorkspaceInfo>,
  group: string,
  sections: SectionInfo[] = [],
): DropPlace {
  const known = new Set(sections.map((s) => s.id));
  const first = topLevel(workspaces, sections).find((w) => groupKey(w, known) === group);
  return group === "pinned"
    ? { before: first?.id ?? null, pinned: true, section: null }
    : { before: first?.id ?? null, pinned: false, section: group === "workspaces" ? null : group };
}

/** The section a section dropped on `target`'s heading goes above (null: last). */
export function sectionDropBefore(sections: SectionInfo[], target: string, below: boolean): string | null {
  const at = sections.findIndex((s) => s.id === target);
  if (at === -1) return null;
  return below ? (sections[at + 1]?.id ?? null) : target;
}
