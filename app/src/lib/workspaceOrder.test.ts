import { describe, expect, test } from "bun:test";
import type { SectionInfo, WorkspaceInfo } from "@chauffeur/protocol";
import {
  dropPlace,
  foldedIndicator,
  headingDropPlace,
  neighbourOf,
  orderedWorkspaces,
  sectionDropBefore,
  shownRows,
  sidebarGroups,
  visibleRows,
} from "./workspaceOrder";

const ws = (id: string, created: string, more: Partial<WorkspaceInfo> = {}) =>
  ({ id, created_at: `2026-10-16T${created}:00Z`, state: "ready", pinned: false, parent_id: null, ...more }) as WorkspaceInfo;
const byId = (...list: WorkspaceInfo[]) => Object.fromEntries(list.map((w) => [w.id, w]));

describe("workspace order", () => {
  test("a workspace being archived has left the sidebar for the archived list", () => {
    const all = byId(ws("a", "10:00"), ws("b", "11:00", { state: "archiving" }), ws("c", "09:00", { state: "archived" }));
    expect(orderedWorkspaces(all, false).map((r) => r.workspace.id)).toEqual(["a"]);
    expect(orderedWorkspaces(all, true).map((r) => r.workspace.id)).toEqual(["b", "c"]);
  });

  test("archives are newest archived first regardless of creation, pins, positions or parent", () => {
    const all = byId(
      ws("parent", "11:00", { state: "archived", pinned: true, position: 100, archived_at: "2026-10-28T12:00:00Z" }),
      ws("child", "08:00", { state: "archived", parent_id: "parent", archived_at: "2026-10-29T12:00:00Z" }),
      ws("middle", "09:00", { state: "archived", archived_at: "2026-10-29T10:00:00Z", section_id: "api" }),
      ws("legacy", "12:00", { state: "archived", archived_at: null }),
    );
    expect(orderedWorkspaces(all, true).map((r) => [r.workspace.id, r.depth])).toEqual([
      ["child", 0], ["middle", 0], ["parent", 0], ["legacy", 0],
    ]);
  });

  test("the selection goes to the next workspace, else the one above, never to a child leaving too", () => {
    // Newest first: c, b (with its child b1), a.
    const all = byId(ws("a", "08:00"), ws("b", "09:00"), ws("b1", "09:30", { parent_id: "b" }), ws("c", "10:00"));
    expect(neighbourOf(all, "b")).toBe("a");
    expect(neighbourOf(all, "a")).toBe("b1");
    expect(neighbourOf(all, "c")).toBe("b");
    expect(neighbourOf(byId(ws("a", "08:00")), "a")).toBeNull();
  });

  test("a workspace already out of the list keeps the selection", () => {
    expect(neighbourOf(byId(ws("a", "08:00"), ws("b", "09:00", { state: "archiving" })), "b")).toBeUndefined();
  });
});

describe("moving workspaces in the sidebar", () => {
  const all = () =>
    byId(
      ws("p", "07:00", { pinned: true, position: 9 }),
      ws("a", "08:00", { position: 3 }),
      ws("b", "09:00", { position: 2 }),
      ws("b1", "09:30", { parent_id: "b", position: 5 }),
      ws("c", "10:00", { position: 1 }),
    );

  test("the user's order wins over the creation date; children stay under their parent", () => {
    expect(orderedWorkspaces(all(), false).map((r) => r.workspace.id)).toEqual(["p", "a", "b", "b1", "c"]);
  });

  test("dropped above a row, below it, or on a child's row", () => {
    expect(dropPlace(all(), "b", false)).toEqual({ before: "b", pinned: false, section: null });
    expect(dropPlace(all(), "a", true)).toEqual({ before: "b", pinned: false, section: null });
    expect(dropPlace(all(), "c", true)).toEqual({ before: null, pinned: false, section: null });
    expect(dropPlace(all(), "b1", false)).toEqual({ before: "c", pinned: false, section: null });
    // Below the last pinned one: last of the pinned, not above the first unpinned.
    expect(dropPlace(all(), "p", true)).toEqual({ before: null, pinned: true, section: null });
  });
});

describe("sections", () => {
  const section = (id: string, folded = false): SectionInfo => ({ id, name: id.toUpperCase(), folded });
  // Pinned p (filed in api, shown in Pinned); api holds a (with its child a1) and b; web holds w;
  // c and d in no section; z is filed in a section that no longer exists.
  const all = () =>
    byId(
      ws("p", "07:00", { pinned: true, position: 9, section_id: "api" }),
      ws("a", "08:00", { position: 3, section_id: "api" }),
      ws("a1", "08:30", { parent_id: "a", position: 8 }),
      ws("b", "09:00", { position: 2, section_id: "api" }),
      ws("w", "09:10", { position: 7, section_id: "web" }),
      ws("c", "10:00", { position: 6 }),
      ws("d", "10:30", { position: 1 }),
      ws("z", "11:00", { position: 5, section_id: "gone" }),
      ws("x", "06:00", { position: 10, section_id: "api", state: "archived" }),
    );
  const sections = (foldApi = false) => [section("api", foldApi), section("web"), section("empty")];
  const ids = (rows: { workspace: WorkspaceInfo }[]) => rows.map((r) => r.workspace.id);

  test("pinned first, then each section in its order, then the others; children under their parent", () => {
    expect(ids(orderedWorkspaces(all(), false, sections()))).toEqual(["p", "a", "a1", "b", "w", "c", "z", "d"]);
    // The archived list is not sectioned.
    expect(ids(orderedWorkspaces(all(), true, sections()))).toEqual(["x"]);
  });

  test("the groups: Pinned, every section even empty, then Workspaces", () => {
    const groups = sidebarGroups(all(), sections());
    expect(groups.map((g) => [g.key, ids(g.rows)])).toEqual([
      ["pinned", ["p"]],
      ["api", ["a", "a1", "b"]],
      ["web", ["w"]],
      ["empty", []],
      ["workspaces", ["c", "z", "d"]],
    ]);
    // Nothing pinned: no Pinned group; Workspaces is always there.
    expect(sidebarGroups(byId(ws("c", "10:00")), []).map((g) => g.key)).toEqual(["workspaces"]);
  });

  test("⌘1–9 follow the rows shown: a folded section is skipped", () => {
    expect(ids(visibleRows(sidebarGroups(all(), sections())))).toEqual(["p", "a", "a1", "b", "w", "c", "z", "d"]);
    expect(ids(visibleRows(sidebarGroups(all(), sections(true))))).toEqual(["p", "w", "c", "z", "d"]);
  });

  test("a folded section still tells when one of its workspaces works or needs input", () => {
    const rows = (...list: WorkspaceInfo[]) => list.map((workspace) => ({ workspace, depth: 0 }));
    expect(foldedIndicator(rows(ws("a", "08:00", { activity: "idle" })))).toEqual({ working: false, needsInput: false });
    expect(
      foldedIndicator(rows(ws("a", "08:00", { activity: "working" }), ws("b", "09:00", { activity: "needs_input" }))),
    ).toEqual({ working: true, needsInput: true });
    // Only a ready workspace's agents count.
    expect(foldedIndicator(rows(ws("a", "08:00", { activity: "working", state: "creating" })))).toEqual({
      working: false,
      needsInput: false,
    });
  });

  test("dropped on a row, a workspace joins that row's section; on a heading, the top of its group", () => {
    expect(dropPlace(all(), "b", false, sections())).toEqual({ before: "b", pinned: false, section: "api" });
    // Below the last of a section: last of it, not above the next section's first.
    expect(dropPlace(all(), "b", true, sections())).toEqual({ before: null, pinned: false, section: "api" });
    expect(dropPlace(all(), "a1", false, sections())).toEqual({ before: "b", pinned: false, section: "api" });
    expect(dropPlace(all(), "c", false, sections())).toEqual({ before: "c", pinned: false, section: null });
    expect(headingDropPlace(all(), "api", sections())).toEqual({ before: "a", pinned: false, section: "api" });
    expect(headingDropPlace(all(), "empty", sections())).toEqual({ before: null, pinned: false, section: "empty" });
    expect(headingDropPlace(all(), "workspaces", sections())).toEqual({ before: "c", pinned: false, section: null });
    expect(headingDropPlace(all(), "pinned", sections())).toEqual({ before: "p", pinned: true, section: null });
  });

  test("a section dropped on another's heading goes above it, or below it", () => {
    expect(sectionDropBefore(sections(), "web", false)).toBe("web");
    expect(sectionDropBefore(sections(), "web", true)).toBe("empty");
    expect(sectionDropBefore(sections(), "empty", true)).toBeNull();
  });

  test("the selection goes to the next workspace in the sidebar's order", () => {
    expect(neighbourOf(all(), "b", sections())).toBe("w");
    expect(neighbourOf(all(), "w", sections())).toBe("c");
  });
});

describe("collapsed parents", () => {
  const all = () => byId(ws("a", "08:00"), ws("b", "09:00"), ws("b1", "09:30", { parent_id: "b" }), ws("b2", "09:40", { parent_id: "b" }), ws("c", "10:00"));

  test("the children of a collapsed parent are left out; the parent and the others stay", () => {
    const rows = orderedWorkspaces(all(), false);
    const ids = (list: typeof rows) => list.map((r) => r.workspace.id);
    expect(ids(shownRows(rows, new Set(["b"])))).toEqual(ids(rows).filter((id) => id !== "b1" && id !== "b2"));
    expect(shownRows(rows, new Set()).map((r) => r.workspace.id)).toEqual(rows.map((r) => r.workspace.id));
  });

  test("⌘1–9 skip the folded children", () => {
    const groups = sidebarGroups(all(), []);
    expect(visibleRows(groups, new Set(["b"])).map((r) => r.workspace.id)).not.toContain("b1");
    expect(visibleRows(groups).map((r) => r.workspace.id)).toContain("b1");
  });
});
