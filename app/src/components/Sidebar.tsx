import type { OpenApp, SectionInfo, WorkspaceInfo } from "@chauffeur/protocol";
import {
  AlarmClock,
  Archive,
  ChevronRight,
  ClipboardCheck,
  Copy,
  CornerDownRight,
  ExternalLink,
  FolderInput,
  FolderOpen,
  FolderPlus,
  GitBranch,
  Layers,
  Mail,
  MailOpen,
  Pencil,
  Pin,
  PinOff,
  Repeat,
  Plus,
  Search,
  Settings,
  Package,
  Telescope,
  Trash2,
  PanelLeftClose,
  ArrowRightLeft,
  Undo2,
  Container,
  SquareCheck,
  Sparkles,
  Users,
  X,
} from "lucide-react";
import { Fragment, memo, useEffect, useMemo, useRef, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { clockTime, workspaceName, workspaceRepoNames, workspaceRepos } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import { routineOfWorkspace, routinesSection, scheduleText, stateText, suggestionsText } from "@/lib/routines";
import {
  type DropPlace,
  dropPlace,
  foldedIndicator,
  headingDropPlace,
  sectionDropBefore,
  shownRows,
  type SidebarGroup,
  type SidebarRow,
  sidebarGroups,
  topLevel,
  visibleRows,
} from "@/lib/workspaceOrder";
import {
  client,
  createSection,
  deleteSection,
  foldSection,
  moveSection,
  openCreate,
  openSettings,
  showSharing,
  openSwitcher,
  renameSection,
  resizeSidebar,
  toggleChildren,
  toggleSidebar,
  pinWorkspace,
  selectWorkspace,
  setUnread,
  setWorkspaceSection,
  showFindRoutines,
  showOffice,
  toast,
  upsertWorkspace,
  useApp,
} from "@/state/app";
import { ArchiveDialog, archiveWorkspace } from "./ArchiveDialog";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { AppIcon } from "./OpenButton";
import { MentionsButton } from "./MentionsButton";
import { PowerIndicator } from "./PowerIndicator";
import { UpdateCard } from "./UpdateCard";
import { SignInBanner } from "./SignInBanner";
import { UsageMeter } from "./UsageMeter";
import { RetitleWorkspaceDialog } from "./RetitleWorkspaceDialog";
import { PromoteDialog } from "./PromoteDialog";
import { DevServicesDialog } from "./DevServicesDialog";
import { WorkspaceShareDialog } from "./WorkspaceShareDialog";
import { chatIdsOf, pendingCount, shareIndicator } from "@/lib/workspaceShare";
import { useWorkspaceShares } from "@/state/workspaceShares";
import { TransferDialog } from "./TransferDialog";
import { devServicesCopy } from "@/lib/devServices";
import { batchCounts, transferCopy, transferUnavailable } from "@/lib/transfer";
import { clearPicked, openBatch, pick, useTransfers } from "@/state/transfers";
import { Badge, Button, cx, IconButton, Kbd, Spinner, StatusDot } from "./ui";
import { kbd } from "../lib/keys";
import { SIDEBAR_DEFAULT_WIDTH } from "@/lib/sidebarWidth";

const DRAG_TYPE = "application/x-chauffeur-workspace";
const SECTION_DRAG_TYPE = "application/x-chauffeur-section";

/** Moves a workspace where it was dropped; the daemon keeps the order and the sections. */
async function moveWorkspace(id: string, place: DropPlace | null) {
  if (!place || place.before === id) return;
  try {
    upsertWorkspace(
      await client.call("workspace.move", {
        workspace: id,
        before: place.before ?? undefined,
        pinned: place.pinned,
        section: place.section ?? undefined,
      }),
    );
  } catch (error) {
    toast(`Could not move it: ${errorMessage(error)}`, "error");
  }
}

/** What is being dragged: a workspace, or a section by its heading. */
type Dragging = { kind: "workspace" | "section"; id: string } | null;
/** Where it would land: above or below a workspace's row, or on a group's heading. */
type Drop = { kind: "row" | "heading"; id: string; below: boolean } | null;

export function Sidebar() {
  const sharingShown = useApp((s) => s.sharingShown);
  const workspaces = useApp(useShallow((s) => s.workspaces));
  const sections = useApp((s) => s.sections);
  const loaded = useApp((s) => s.workspacesLoaded);
  const connection = useApp((s) => s.connection);
  const officeEnabled = useApp((s) => s.officeEnabled);
  const sidebarWidth = useApp((s) => s.sidebarWidth);
  const groups = useMemo(() => sidebarGroups(workspaces, sections), [workspaces, sections]);
  const routines = useApp((s) => s.routines);
  const routinesSectionId = useMemo(() => routinesSection(routines, workspaces), [routines, workspaces]);
  const activeCount = groups.reduce((n, g) => n + g.rows.length, 0);
  // ⌘1–9 follow the rows shown: folded sections and folded children are skipped.
  const collapsedList = useApp((s) => s.collapsedParents);
  const collapsed = useMemo(() => new Set(collapsedList), [collapsedList]);
  // The child workspaces of each parent shown, for its fold button.
  const childrenOf = useMemo(() => {
    const map = new Map<string, SidebarRow[]>();
    for (const group of groups)
      for (const row of group.rows)
        if (row.depth > 0 && row.workspace.parent_id) map.set(row.workspace.parent_id, [...(map.get(row.workspace.parent_id) ?? []), row]);
    return map;
  }, [groups]);
  const shown = useMemo(() => visibleRows(groups, collapsed).map((r) => r.workspace.id), [groups, collapsed]);
  const shortcuts = useMemo(() => new Map(shown.slice(0, 9).map((id, i) => [id, i + 1])), [shown]);
  // Several workspaces picked (⌘-click, ⇧-click, or Select in a row's menu) act together.
  const picked = useTransfers((s) => s.picked);
  const pickedWorkspaces = useMemo(() => picked.map((id) => workspaces[id]).filter((w): w is WorkspaceInfo => !!w), [picked, workspaces]);
  useEffect(() => {
    if (!picked.length) return;
    // Before the open chat's own Escape (a question's card takes it): the selection goes first,
    // unless a dialog or a menu is open, or the user types elsewhere.
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || document.querySelector("[role=dialog], [role=menu]")) return;
      const focus = document.activeElement;
      if (focus && focus !== document.body && (!focus.closest("[data-sidebar]") || focus.matches("input, textarea, select"))) return;
      event.stopPropagation();
      clearPicked();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [picked.length]);
  // Archived workspaces stay out of the list: the footer's button finds them.
  const archivedCount = useMemo(() => Object.values(workspaces).filter((w) => w.state === "archived" || w.state === "archiving").length, [workspaces]);
  // Archived at once; the dialog only opens with the daemon's refusal.
  const [archiving, setArchiving] = useState<{ workspace: WorkspaceInfo; refusal: string } | null>(null);
  const [promoting, setPromoting] = useState<WorkspaceInfo | null>(null);
  const [transferring, setTransferring] = useState<WorkspaceInfo[] | null>(null);
  const [servicesOf, setServicesOf] = useState<WorkspaceInfo | null>(null);
  const [sharing, setSharing] = useState<WorkspaceInfo | null>(null);
  // The name of a new section, typed in place; for a workspace to file in it, when asked from its menu.
  const [newSection, setNewSection] = useState<{ workspace?: string } | null>(null);
  // The rows are memoised: their actions take the workspace and stay the same objects, reading the
  // selection as it is when they run.
  const selection = useRef({ picked, pickedWorkspaces, shown });
  selection.current = { picked, pickedWorkspaces, shown };
  const actions = useMemo<RowActions>(
    () => ({
      archive: (workspace) =>
        void archiveWorkspace(workspace).then((refusal) => {
          if (refusal) setArchiving({ workspace, refusal });
        }),
      promote: setPromoting,
      pick: (workspace, range) => pick(workspace.id, range, selection.current.shown),
      transfer: (workspace) => {
        const { picked, pickedWorkspaces } = selection.current;
        setTransferring(picked.length > 1 && picked.includes(workspace.id) ? pickedWorkspaces : [workspace]);
      },
      services: setServicesOf,
      share: setSharing,
      newSection: (workspace) => setNewSection({ workspace: workspace.id }),
    }),
    [],
  );
  useEffect(() => {
    if (connection.status === "open" && !openApps) loadOpenApps();
  }, [connection.status]);
  // Drag a top-level workspace above or below another to reorder the sidebar; across groups, it is
  // pinned, unpinned or filed in another section; onto a heading, it goes on top of that group.
  // Drag a section's heading to reorder the sections.
  const movable = useMemo(() => new Set(topLevel(workspaces, sections).map((w) => w.id)), [workspaces, sections]);
  const [dragging, setDragging] = useState<Dragging>(null);
  const [drop, setDrop] = useState<Drop>(null);
  const endDrag = () => {
    setDragging(null);
    setDrop(null);
  };
  const onDropped = () => {
    const [moved, target] = [dragging, drop];
    endDrag();
    if (!moved || !target) return;
    if (moved.kind === "section") {
      if (target.kind !== "heading" || target.id === moved.id) return;
      const before = sectionDropBefore(sections, target.id, target.below);
      if (before !== moved.id) void moveSection(moved.id, before);
    } else if (target.kind === "row") {
      void moveWorkspace(moved.id, dropPlace(workspaces, target.id, target.below, sections));
    } else {
      void moveWorkspace(moved.id, headingDropPlace(workspaces, target.id, sections));
    }
  };
  /** Drag handlers of a group's heading: a workspace lands on top of the group, a section above or below another one. */
  const headingDrop = (key: string, isSection: boolean) => ({
    onDragOver: (event: React.DragEvent<HTMLElement>) => {
      if (!dragging || (dragging.kind === "section" && !isSection)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "move";
      const rect = event.currentTarget.getBoundingClientRect();
      const below = dragging.kind === "section" && event.clientY > rect.top + rect.height / 2;
      if (drop?.kind !== "heading" || drop.id !== key || drop.below !== below) setDrop({ kind: "heading", id: key, below });
    },
    onDrop: (event: React.DragEvent<HTMLElement>) => {
      event.preventDefault();
      onDropped();
    },
  });
  const headingTarget = (key: string) => (drop?.kind === "heading" && drop.id === key ? drop : null);

  const renderRow = ({ workspace, depth }: SidebarRow) => (
    <div
      key={workspace.id}
      className="relative"
      draggable={movable.has(workspace.id)}
      onDragStart={(event) => {
        event.stopPropagation();
        event.dataTransfer.effectAllowed = "move";
        event.dataTransfer.setData(DRAG_TYPE, workspace.id);
        setDragging({ kind: "workspace", id: workspace.id });
      }}
      onDragEnd={endDrag}
      onDragOver={(event) => {
        if (dragging?.kind !== "workspace") return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "move";
        const rect = event.currentTarget.getBoundingClientRect();
        const below = event.clientY > rect.top + rect.height / 2;
        if (drop?.kind !== "row" || drop.id !== workspace.id || drop.below !== below) setDrop({ kind: "row", id: workspace.id, below });
      }}
      onDrop={(event) => {
        event.preventDefault();
        onDropped();
      }}
      data-workspace-row={workspace.id}
    >
      {drop?.kind === "row" && drop.id === workspace.id && dragging?.id !== workspace.id && (
        <span
          className={cx("pointer-events-none absolute inset-x-1 z-10 h-0.5 rounded-full bg-accent", drop.below ? "-bottom-px" : "-top-px")}
        />
      )}
      <div className={cx(dragging?.id === workspace.id && "opacity-40")}>
        <WorkspaceRow
          workspace={workspace}
          depth={depth}
          shortcut={shortcuts.get(workspace.id) ?? null}
          children={childrenOf.get(workspace.id) ?? null}
          childrenFolded={collapsed.has(workspace.id)}
          sections={sections}
          picked={picked.includes(workspace.id)}
          pickedCount={picked.length}
          actions={actions}
        />
      </div>
    </div>
  );

  const renderGroup = (group: SidebarGroup) => {
    if (group.kind === "section") {
      return (
        <Fragment key={group.key}>
          <SectionHeading
            section={group.section}
            rows={group.rows}
            dragging={dragging?.kind === "section" && dragging.id === group.key}
            target={headingTarget(group.key)}
            draggingKind={dragging?.kind ?? null}
            onDragStart={(event) => {
              event.dataTransfer.effectAllowed = "move";
              event.dataTransfer.setData(SECTION_DRAG_TYPE, group.key);
              setDragging({ kind: "section", id: group.key });
            }}
            onDragEnd={endDrag}
            routines={group.key === routinesSectionId}
            {...headingDrop(group.key, true)}
          />
          {group.key === routinesSectionId && <SuggestionsEntry />}
          {!group.section.folded && shownRows(group.rows, collapsed).map(renderRow)}
        </Fragment>
      );
    }
    const title = group.kind === "pinned" ? "Pinned" : "Workspaces";
    return (
      <Fragment key={group.key}>
        {group.kind === "workspaces" && !routinesSectionId && <RoutinesHeading />}
        {group.kind === "workspaces" && newSection && (
          <NewSectionField workspace={newSection.workspace} onDone={() => setNewSection(null)} />
        )}
        <GroupHeading
          title={title}
          highlighted={dragging?.kind === "workspace" && !!headingTarget(group.key)}
          onNewSection={group.kind === "workspaces" ? () => setNewSection({}) : undefined}
          {...headingDrop(group.key, false)}
        />
        {group.kind === "workspaces" && activeCount === 0 ? (
          <button
            onClick={() => openCreate()}
            className="mx-1 mt-1 flex w-[calc(100%-8px)] items-center justify-between rounded-md border border-dashed border-line px-2.5 py-2 text-[12px] text-faint hover:border-line-strong hover:text-muted"
          >
            New workspace <Kbd>{kbd("⌘N")}</Kbd>
          </button>
        ) : (
          shownRows(group.rows, collapsed).map(renderRow)
        )}
      </Fragment>
    );
  };

  return (
    <aside
      className="relative flex h-full shrink-0 flex-col border-r border-line bg-sidebar"
      style={{ width: sidebarWidth }}
      data-sidebar
    >
      <div className="flex h-11 shrink-0 items-center justify-between pl-4 pr-2" data-tauri-drag-region>
        <div className="flex items-center gap-2" data-tauri-drag-region>
          <span className="font-semibold tracking-tight text-fg">Chauffeur</span>
          <ConnectionDot status={connection.status} />
        </div>
        <div className="flex items-center gap-0.5">
          <IconButton title={`Hide the sidebar (${kbd("⌘B")})`} onClick={() => toggleSidebar(true)}>
            <PanelLeftClose size={14} />
          </IconButton>
          <IconButton title={`Switch workspace (${kbd("⌘K")})`} onClick={() => openSwitcher()}>
            <Search size={14} />
          </IconButton>
          <IconButton title={`New exploration, read-only (${kbd("⇧⌘N")})`} onClick={() => openCreate(true, true)}>
            <Telescope size={14} />
          </IconButton>
          <IconButton title={`New workspace (${kbd("⌘N")})`} onClick={() => openCreate()}>
            <Plus size={15} />
          </IconButton>
        </div>
      </div>

      <nav className="min-h-0 flex-1 overflow-y-auto px-2 pb-3" aria-label="Workspaces">
        {!loaded && connection.status !== "open" ? (
          <>
            <SectionTitle>Workspaces</SectionTitle>
            <div className="px-2 py-2 text-[12px] text-faint">Connecting…</div>
          </>
        ) : (
          groups.map(renderGroup)
        )}
      </nav>
      {pickedWorkspaces.length > 0 && (
        <SelectionBar count={pickedWorkspaces.length} onTransfer={() => setTransferring(pickedWorkspaces)} />
      )}
      <button onClick={showSharing} data-testid="sharing-button" className={cx("mx-2 mb-2 flex items-center gap-2 rounded-md px-2 py-2 text-xs hover:bg-hover", sharingShown ? "bg-hover text-accent" : "text-muted")}>
        <Users size={14} /> Sharing
      </button>
      <UsageMeter />
      <UpdateCard />
      <SignInBanner />
      <div className="flex shrink-0 items-center justify-between border-t border-line px-2 py-1.5">
        <span className="flex items-center gap-0.5">
          <IconButton title={`Settings (${kbd("⌘,")})`} onClick={() => openSettings()}>
            <Settings size={14} />
          </IconButton>
          <IconButton title="Extension marketplace" onClick={() => openSettings(true, "extensions")} data-testid="open-extensions">
            <Package size={14} />
          </IconButton>
          {officeEnabled && <OfficeButton />}
          <MentionsButton />
          <PowerIndicator />
          <BatchPill />
        </span>
        {archivedCount > 0 && (
          <button
            onClick={() => openSwitcher(true, true)}
            title="Find an archived workspace, to read or restore it"
            className="flex h-6 items-center gap-1.5 rounded-md px-1.5 text-[11.5px] text-faint hover:bg-hover hover:text-muted"
            data-testid="archived-button"
          >
            <Archive size={12} /> Archived ({archivedCount})
          </button>
        )}
      </div>
      {archiving && (
        <ArchiveDialog open onClose={() => setArchiving(null)} workspace={archiving.workspace} refusal={archiving.refusal} />
      )}
      {promoting && <PromoteDialog open onClose={() => setPromoting(null)} workspace={promoting} />}
      {transferring && <TransferDialog workspaces={transferring} onClose={() => setTransferring(null)} />}
      {servicesOf && <DevServicesDialog workspace={servicesOf} onClose={() => setServicesOf(null)} />}
      {sharing && <WorkspaceShareDialog workspace={sharing} onClose={() => setSharing(null)} />}
      <SidebarEdge />
    </aside>
  );
}

/** The sidebar's right edge: dragged to resize it, double-clicked to give back its default width. */
function SidebarEdge() {
  const [resizing, setResizing] = useState(false);
  // The pointer's distance from the sidebar's left edge, in CSS pixels whatever the page's zoom.
  const widthAt = (event: React.PointerEvent<HTMLDivElement>) => {
    const aside = event.currentTarget.parentElement;
    if (!aside) return null;
    const rect = aside.getBoundingClientRect();
    return rect.width > 0 ? ((event.clientX - rect.left) * aside.offsetWidth) / rect.width : null;
  };
  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize the sidebar"
      title="Drag to resize, double-click to reset"
      className="group absolute inset-y-0 -right-[3px] z-20 w-[6px] cursor-col-resize touch-none"
      onPointerDown={(event) => {
        if (event.button !== 0) return;
        event.preventDefault();
        event.currentTarget.setPointerCapture(event.pointerId);
        setResizing(true);
      }}
      onPointerMove={(event) => {
        if (!resizing) return;
        const width = widthAt(event);
        if (width != null) resizeSidebar(width, false);
      }}
      onPointerUp={(event) => {
        if (!resizing) return;
        setResizing(false);
        const width = widthAt(event);
        if (width != null) resizeSidebar(width, true);
      }}
      onPointerCancel={() => setResizing(false)}
      onDoubleClick={() => resizeSidebar(SIDEBAR_DEFAULT_WIDTH, true)}
      data-testid="sidebar-edge"
    >
      <div
        className={cx(
          "absolute inset-y-0 left-1/2 w-px -translate-x-1/2 transition-colors",
          resizing ? "bg-accent" : "group-hover:bg-accent/60",
        )}
      />
    </div>
  );
}

/** The workspaces picked in the list, and what they can do together. */
function SelectionBar({ count, onTransfer }: { count: number; onTransfer: () => void }) {
  const copy = transferCopy(navigator.language.startsWith("fr"));
  return (
    <div
      className="mx-2 mb-1.5 flex shrink-0 items-center gap-1 rounded-md border border-accent/30 bg-accent/5 py-1 pl-2.5 pr-1 text-[12px]"
      data-testid="selection-bar"
    >
      <span className="min-w-0 flex-1 truncate text-fg" aria-live="polite">
        {copy.picked(count)}
      </span>
      <Button size="sm" variant="primary" onClick={onTransfer} data-testid="selection-transfer">
        <ArrowRightLeft size={12} /> {copy.transferItem}
      </Button>
      <IconButton title={copy.clearPicked} onClick={clearPicked} data-testid="selection-clear">
        <X size={13} />
      </IconButton>
    </div>
  );
}

/** A batch of transfers kept after its view closed: how far it is, a click to see it again. */
function BatchPill() {
  const batch = useTransfers((s) => s.batch);
  const open = useTransfers((s) => s.batchOpen);
  if (!batch || open) return null;
  const copy = transferCopy(navigator.language.startsWith("fr"));
  const counts = batchCounts(batch.items);
  return (
    <button
      onClick={() => openBatch(true)}
      title={copy.batchPillTitle}
      className={cx(
        "flex h-6 items-center gap-1 rounded-md px-1.5 text-[11px] hover:bg-hover",
        counts.failed ? "text-danger" : "text-muted hover:text-fg",
      )}
      data-testid="transfer-batch-pill"
    >
      {counts.running > 0 ? <Spinner size={10} /> : <ArrowRightLeft size={11} />}
      {copy.batchPill(batch.items.length - counts.running, batch.items.length)}
    </button>
  );
}

/** Shows the 3D Office in the main area, or the workspace again. */
function OfficeButton() {
  const shown = useApp((s) => s.officeShown);
  return (
    <button
      onClick={() => showOffice()}
      title={shown ? `Back to the workspace (${kbd("⇧⌘O")})` : `3D Office: the village of your team's agents (${kbd("⇧⌘O")})`}
      aria-label={shown ? "Back to the workspace" : "3D Office"}
      aria-pressed={shown}
      className={cx(
        "inline-flex h-6 items-center justify-center rounded-md px-1.5 text-[11px] font-semibold tracking-tight",
        shown ? "bg-hover text-fg" : "text-muted hover:bg-hover hover:text-fg",
      )}
      data-testid="office-button"
    >
      3D
    </button>
  );
}

function ConnectionDot({ status }: { status: "connecting" | "open" | "closed" }) {
  return (
    <span
      title={status === "open" ? "Connected to chauffeurd" : "Not connected to chauffeurd"}
      className={cx(
        "h-1.5 w-1.5 rounded-full",
        status === "open" ? "bg-success" : status === "connecting" ? "bg-attention animate-pulse-dot" : "bg-danger",
      )}
    />
  );
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return <div className="px-2 pb-1 pt-2 text-[11px] font-medium uppercase tracking-wide text-faint">{children}</div>;
}

type DropHandlers = {
  onDragOver: (event: React.DragEvent<HTMLElement>) => void;
  onDrop: (event: React.DragEvent<HTMLElement>) => void;
};

/** "Pinned" or "Workspaces": a workspace dropped on it goes on top; "Workspaces" offers a new section on hover. */
function GroupHeading({
  title,
  highlighted,
  onNewSection,
  ...drop
}: { title: string; highlighted: boolean; onNewSection?: () => void } & DropHandlers) {
  return (
    <div
      {...drop}
      data-group-heading={title}
      className={cx("group/heading flex items-center justify-between rounded-md", highlighted && "bg-accent/10 ring-1 ring-accent/40")}
    >
      <SectionTitle>{title}</SectionTitle>
      {onNewSection && (
        <span className="mr-1 mt-1 hidden group-hover/heading:block">
          <IconButton title="New section, to file workspaces by theme" className="h-5 w-5" onClick={onNewSection} data-testid="new-section">
            <FolderPlus size={12} />
          </IconButton>
        </span>
      )}
    </div>
  );
}

/**
 * A section's heading: its chevron folds it (remembered by the daemon), its count, and while
 * folded what its workspaces still need to tell (one working, one needing input). Dragged to
 * reorder the sections; a workspace dropped on it goes on top of the section.
 */
function SectionHeading({
  section,
  rows,
  dragging,
  draggingKind,
  target,
  onDragStart,
  onDragEnd,
  routines = false,
  ...drop
}: {
  section: SectionInfo;
  rows: SidebarRow[];
  dragging: boolean;
  draggingKind: "workspace" | "section" | null;
  target: { below: boolean } | null;
  onDragStart: (event: React.DragEvent<HTMLElement>) => void;
  onDragEnd: () => void;
  /** The routines' section: it holds Find routines. */
  routines?: boolean;
} & DropHandlers) {
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState(false);
  const indicator = section.folded ? foldedIndicator(rows) : null;
  const toggle = () => void foldSection(section.id, !section.folded);
  return (
    <div
      className="relative mt-1"
      draggable={!renaming}
      onDragStart={onDragStart}
      onDragEnd={onDragEnd}
      {...drop}
      data-section={section.name}
    >
      {target && draggingKind === "section" && !dragging && (
        <span className={cx("pointer-events-none absolute inset-x-1 z-10 h-0.5 rounded-full bg-accent", target.below ? "-bottom-px" : "-top-px")} />
      )}
      <div
        onContextMenu={(event) => {
          event.preventDefault();
          setMenu({ x: event.clientX, y: event.clientY });
        }}
        className={cx(
          "flex h-7 items-center gap-1.5 rounded-md pl-1 pr-2 text-[11px] font-medium uppercase tracking-wide text-faint hover:text-muted",
          target && draggingKind === "workspace" && "bg-accent/10 ring-1 ring-accent/40",
          dragging && "opacity-40",
        )}
      >
        {renaming ? (
          <>
            <ChevronRight size={12} className={cx("shrink-0", !section.folded && "rotate-90")} />
            <NameField
              initial={section.name}
              onSave={(name) => (name === section.name ? undefined : renameSection(section.id, name))}
              onDone={() => setRenaming(false)}
            />
          </>
        ) : (
          <button
            type="button"
            onClick={toggle}
            title={section.folded ? "Unfold" : "Fold"}
            aria-expanded={!section.folded}
            className="flex min-w-0 flex-1 items-center gap-1 text-left uppercase"
          >
            <ChevronRight size={12} className={cx("shrink-0 transition-transform", !section.folded && "rotate-90")} />
            <span className="truncate">{section.name}</span>
          </button>
        )}
        {indicator?.needsInput && (
          <span title="A workspace in it needs input" aria-label="A workspace in it needs input" className="h-1.5 w-1.5 shrink-0 rounded-full bg-attention" />
        )}
        {indicator?.working && (
          <span title="A workspace in it is working" aria-label="A workspace in it is working" className="shrink-0 text-working">
            <Spinner size={10} />
          </span>
        )}
        {routines && <FindRoutinesButton />}
        <span className="shrink-0 text-[10.5px] font-normal tabular-nums normal-case" data-testid="section-count">
          {rows.length}
        </span>
      </div>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={[
            { label: "Rename", icon: <Pencil size={13} />, shortcut: "r", onSelect: () => setRenaming(true) },
            { label: section.folded ? "Unfold" : "Fold", icon: <ChevronRight size={13} />, shortcut: "f", onSelect: toggle },
            "separator",
            {
              label: "Delete section",
              icon: <Trash2 size={13} />,
              danger: true,
              hint: "Its workspaces go back to Workspaces",
              onSelect: () => void deleteSection(section),
            },
          ]}
          onClose={() => setMenu(null)}
        />
      )}
    </div>
  );
}

/** Opens Find routines (docs/adr/find-routines.md), in the routines' section heading. */
function FindRoutinesButton() {
  return (
    <IconButton
      title="Find routines: look through your recent requests for work worth running by itself"
      className="h-5 w-5 normal-case"
      onClick={() => showFindRoutines(true)}
      data-testid="find-routines-button"
    >
      <Sparkles size={12} />
    </IconButton>
  );
}

/** "Routines" before any routine is filed in a section: where Find routines is. */
function RoutinesHeading() {
  return (
    <>
      <div className="flex items-center justify-between pr-2" data-group-heading="Routines">
        <SectionTitle>Routines</SectionTitle>
        <span className="mt-1">
          <FindRoutinesButton />
        </span>
      </div>
      <SuggestionsEntry />
    </>
  );
}

/** The suggestions Find routines made, kept under the routines' heading until acted on. */
function SuggestionsEntry() {
  const find = useApp((s) => s.routineFind);
  const text = suggestionsText(find);
  if (!text) return null;
  const fresh = find?.suggestions.some((s) => s.state === "new") ?? false;
  return (
    <button
      type="button"
      onClick={() => showFindRoutines(true)}
      className="mb-0.5 flex h-7 w-full items-center gap-2 rounded-md px-2 text-left text-[12.5px] text-muted hover:bg-panel hover:text-fg"
      data-testid="routine-suggestions"
    >
      {find?.running ? <Spinner size={11} /> : <Sparkles size={12} className="shrink-0 text-accent" />}
      <span className="min-w-0 flex-1 truncate">{text}</span>
      {fresh && !find?.running && <span aria-label="New suggestions" className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />}
    </button>
  );
}

/** A new section's name, typed in place above "Workspaces": Enter or leaving creates it, Escape cancels. */
function NewSectionField({ workspace, onDone }: { workspace?: string; onDone: () => void }) {
  return (
    <div className="mt-1 flex h-7 items-center gap-1.5 pl-1 pr-2 text-faint">
      <FolderPlus size={12} className="shrink-0" />
      <NameField
        initial=""
        placeholder={workspace ? "Section name, to file it in" : "Section name"}
        onSave={async (name) => (name ? await createSection(name, workspace) : true)}
        saveOnBlur
        onDone={onDone}
        testId="new-section-field"
      />
    </div>
  );
}

/** A name edited in place: Enter saves (the field stays when saving fails), Escape cancels. */
function NameField({
  initial,
  placeholder,
  onSave,
  onDone,
  saveOnBlur = false,
  testId = "section-name-field",
}: {
  initial: string;
  placeholder?: string;
  /** False keeps the field open. */
  onSave: (name: string) => unknown;
  onDone: () => void;
  saveOnBlur?: boolean;
  testId?: string;
}) {
  const [value, setValue] = useState(initial);
  const input = useRef<HTMLInputElement>(null);
  const settled = useRef(false);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  const save = async () => {
    if (settled.current) return;
    settled.current = true;
    if ((await onSave(value.trim())) === false) {
      settled.current = false;
      input.current?.focus();
      return;
    }
    onDone();
  };
  return (
    <input
      ref={input}
      value={value}
      placeholder={placeholder}
      onChange={(event) => setValue(event.target.value)}
      onClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Enter") void save();
        if (event.key === "Escape") {
          settled.current = true;
          onDone();
        }
      }}
      onBlur={() => {
        if (settled.current) return;
        if (saveOnBlur && value.trim()) void save();
        else {
          settled.current = true;
          onDone();
        }
      }}
      maxLength={60}
      className="min-w-0 flex-1 rounded border border-line-strong bg-bg px-1 text-[12px] normal-case tracking-normal text-fg outline-none"
      data-testid={testId}
    />
  );
}

/** What a row's clicks and menu do, given its workspace. */
type RowActions = {
  archive: (workspace: WorkspaceInfo) => void;
  promote: (workspace: WorkspaceInfo) => void;
  /** ⌘-click picks it or not; ⇧-click (`range`) picks the rows up to it. */
  pick: (workspace: WorkspaceInfo, range: boolean) => void;
  transfer: (workspace: WorkspaceInfo) => void;
  services: (workspace: WorkspaceInfo) => void;
  share: (workspace: WorkspaceInfo) => void;
  newSection: (workspace: WorkspaceInfo) => void;
};

const WorkspaceRow = memo(function WorkspaceRow({
  workspace,
  depth,
  shortcut,
  children,
  childrenFolded,
  sections,
  picked,
  pickedCount,
  actions,
}: {
  workspace: WorkspaceInfo;
  /** Picked among several, to act on them together. */
  picked: boolean;
  pickedCount: number;
  depth: number;
  shortcut: number | null;
  /** Its child workspaces, when it has some: a button folds them. */
  children: SidebarRow[] | null;
  childrenFolded: boolean;
  sections: SectionInfo[];
  actions: RowActions;
}) {
  const onPick = (range: boolean) => actions.pick(workspace, range);
  const onArchive = () => actions.archive(workspace);
  // In the Office, the workspace whose chat is open beside the scene.
  const selected = useApp((s) => (s.officeShown ? s.officeChat?.workspace === workspace.id : s.selectedId === workspace.id));
  const routine = useApp((s) => routineOfWorkspace(s.routines, workspace.id));
  // Marked unread from its menu, the workspace shown says so too.
  const unread = !!workspace.unread;
  const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
  const [renaming, setRenaming] = useState(false);
  const [autoRenaming, setAutoRenaming] = useState(false);
  const repos = workspace.state === "archived" ? workspaceRepoNames(workspace) : workspaceRepos(workspace);
  const ticket = workspace.tickets[0]?.identifier;
  // A moved workspace comes back before it is archived: the daemon brings it back, then archives it.
  const archivable = workspace.state === "ready" || workspace.state === "cleanup_failed";
  const indicator = children && childrenFolded ? foldedIndicator(children) : null;
  return (
    <div className="group relative">
      {/* A div while renaming: in a button, the field's spaces would press it. */}
      <RowTag
        renaming={renaming}
        onMouseDown={(event) => {
          // ⇧-click picks rows, it does not select their text.
          if (event.shiftKey) event.preventDefault();
        }}
        onClick={(event) => {
          if (event.metaKey || event.ctrlKey || event.shiftKey) {
            onPick(event.shiftKey);
            return;
          }
          clearPicked();
          selectWorkspace(workspace.id);
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          setMenu({ x: event.clientX, y: event.clientY });
        }}
        data-workspace={workspace.slug}
        aria-current={selected ? "page" : undefined}
        aria-pressed={pickedCount > 0 ? picked : undefined}
        data-picked={picked ? "" : undefined}
        className={cx(
          "group relative flex w-full flex-col rounded-md py-1.5 text-left",
          picked ? "bg-accent/10 ring-1 ring-inset ring-accent/35" : selected ? "bg-hover" : "hover:bg-hover/60",
          // Extensions load their own px-2 utility later; it would overwrite a child's pl-6.
          depth > 0 ? "pl-6 pr-2" : "px-2",
        )}
      >
        <div className={cx("flex w-full items-center gap-2", children && "pr-7")}>
          {depth > 0 ? (
            <CornerDownRight size={11} className="-ml-3.5 shrink-0 text-faint" />
          ) : null}
          <StatusDot workspace={workspace} />
          {renaming ? (
            <RenameField workspace={workspace} onDone={() => setRenaming(false)} />
          ) : (
            <span
              className={cx(
                "min-w-0 flex-1 truncate",
                selected || unread ? "text-fg" : "text-muted",
                unread && "font-semibold",
                workspace.state === "archived" && "text-faint",
              )}
            >
              {workspaceName(workspace)}
            </span>
          )}
          {workspace.pinned && depth === 0 && <Pin size={10} className="shrink-0 text-faint" aria-label="Pinned" />}
          {unread && <span title="Unread" className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />}
          <TasksCount count={workspace.running_tasks ?? 0} />
          <ShareMark workspaceId={workspace.id} />
          <RowBadge workspace={workspace} />
          {shortcut != null && (
            <span className="hidden text-[10.5px] text-faint group-hover:inline">{kbd(`⌘${shortcut}`)}</span>
          )}
        </div>
        <div className="ml-4 flex min-w-0 items-center gap-1.5 text-[11px] text-faint">
          {routine ? (
            <Badge
              tone={routine.state === "active" ? "success" : routine.state === "paused" ? "attention" : "neutral"}
              title={`Routine: ${scheduleText(routine.spec.schedule)}${routine.state === "active" ? "" : ", runs only by hand until activated"}`}
              className="shrink-0"
            >
              <Repeat size={10} /> {stateText(routine, new Date())}
            </Badge>
          ) : workspace.daily_review_date ? (
            <Badge
              tone="accent"
              title={`Automatic session review for ${workspace.daily_review_date}${workspace.explore ? " · read-only exploration" : ""}`}
              className="shrink-0"
            >
              <ClipboardCheck size={10} /> daily review
            </Badge>
          ) : workspace.explore && (
            <span title="Exploration: read-only, at each repo's base branch" className="inline-flex shrink-0 items-center gap-1 text-muted/80">
              <Telescope size={10} /> explore
            </span>
          )}
          {ticket && <span className="shrink-0 text-muted/80">{ticket}</span>}
          <span className={cx("truncate", archivable && "group-hover:pr-6")}>{repos.join(" · ") || "no repos"}</span>
        </div>
      </RowTag>
      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={rowMenu(workspace, {
            picked,
            pickedCount,
            onPick: () => onPick(false),
            archivable,
            onArchive,
            onPromote: () => actions.promote(workspace),
            onTransfer: () => actions.transfer(workspace),
            onServices: () => actions.services(workspace),
            onShare: () => actions.share(workspace),
            onRename: () => setRenaming(true),
            onAutoRename: () => setAutoRenaming(true),
            // A child shown under its parent follows it.
            sections: depth === 0 ? sections : null,
            onNewSection: () => actions.newSection(workspace),
          })}
          onClose={() => setMenu(null)}
        />
      )}
      {autoRenaming && <RetitleWorkspaceDialog workspace={workspace} onClose={() => setAutoRenaming(false)} />}
      {children && (
        <button
          type="button"
          onClick={() => toggleChildren(workspace.id)}
          title={childrenFolded ? `Show its ${children.length} sub-workspaces` : "Hide its sub-workspaces"}
          aria-label={childrenFolded ? "Show sub-workspaces" : "Hide sub-workspaces"}
          aria-expanded={!childrenFolded}
          data-testid="row-children-toggle"
          className="absolute right-1 top-1.5 flex h-5 items-center gap-0.5 rounded px-0.5 text-[10.5px] tabular-nums text-faint hover:bg-hover hover:text-fg"
        >
          {indicator?.needsInput && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-attention" aria-label="A sub-workspace needs input" />}
          {indicator?.working && (
            <span className="shrink-0 text-working" aria-label="A sub-workspace is working">
              <Spinner size={10} />
            </span>
          )}
          {childrenFolded && <span data-testid="row-children-count">{children.length}</span>}
          <ChevronRight size={12} className={cx("shrink-0 transition-transform", !childrenFolded && "rotate-90")} />
        </button>
      )}
      {archivable && (
        <span className="absolute right-1 bottom-0.5 hidden group-hover:block">
          <IconButton title="Archive" onClick={onArchive} data-testid="row-archive">
            <Archive size={12} />
          </IconButton>
        </span>
      )}
    </div>
  );
});

/** The row's clickable body: a button, or a div while its name is edited. */
function RowTag({
  renaming,
  children,
  ...props
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { renaming: boolean } & Record<`data-${string}`, string | undefined>) {
  if (renaming) {
    const { className, "aria-current": current } = props;
    return (
      <div className={className} aria-current={current} data-workspace={props["data-workspace"]}>
        {children}
      </div>
    );
  }
  return (
    <button type="button" {...props}>
      {children}
    </button>
  );
}

/** The right-click menu of a sidebar row. */
function rowMenu(
  workspace: WorkspaceInfo,
  actions: {
    picked: boolean;
    pickedCount: number;
    onPick: () => void;
    archivable: boolean;
    onArchive: () => void;
    onPromote: () => void;
    onTransfer: () => void;
    onServices: () => void;
    onShare: () => void;
    onRename: () => void;
    onAutoRename: () => void;
    sections: SectionInfo[] | null;
    onNewSection: () => void;
  },
): MenuItem[] {
  const ready = workspace.state === "ready";
  const items: MenuItem[] = [
    workspace.unread
      ? { label: "Mark as read", icon: <MailOpen size={13} />, shortcut: "r", onSelect: () => void setUnread(workspace.id, false) }
      : { label: "Mark as unread", icon: <Mail size={13} />, shortcut: "r", onSelect: () => void setUnread(workspace.id, true) },
    workspace.pinned
      ? { label: "Unpin", icon: <PinOff size={13} />, shortcut: "p", onSelect: () => void pinWorkspace(workspace.id, false) }
      : { label: "Pin", icon: <Pin size={13} />, shortcut: "p", onSelect: () => void pinWorkspace(workspace.id, true) },
    { label: "Rename", icon: <Pencil size={13} />, onSelect: actions.onRename },
    { label: "Auto-rename…", icon: <Sparkles size={13} />, disabled: !ready, hint: ready ? "Name from recent messages" : "Available once the workspace is ready", onSelect: actions.onAutoRename },
  ];
  if (actions.sections) {
    items.push({ label: "Move to section", icon: <FolderInput size={13} />, submenu: sectionSubmenu(workspace, actions.sections, actions.onNewSection) });
  }
  // A moved workspace is read-only here: it comes back before anything else.
  const here = ready && !workspace.moved_to;
  if (here) items.push({ label: "Open in", icon: <ExternalLink size={13} />, submenu: openSubmenu(workspace) });
  if (ready) items.push({ label: "Share workspace…", icon: <Users size={13} />, onSelect: actions.onShare });
  items.push("separator");
  if (workspace.branch) {
    items.push({ label: "Copy branch name", icon: <GitBranch size={13} />, onSelect: () => void copy(workspace.branch, "Branch name") });
  }
  items.push({ label: "Copy path", icon: <Copy size={13} />, onSelect: () => void copy(workspace.path, "Path") });
  if (workspace.explore && here) {
    items.push("separator", { label: "Continue as workspace", icon: <GitBranch size={13} />, onSelect: actions.onPromote });
  }
  const french = navigator.language.startsWith("fr"),
    transfer = transferCopy(french);
  items.push({
    label: actions.picked ? transfer.deselect : transfer.select,
    icon: <SquareCheck size={13} />,
    shortcut: "s",
    hint: "⌘-click",
    onSelect: actions.onPick,
  });
  if (actions.picked && actions.pickedCount > 1) {
    items.push("separator", {
      label: transfer.transferMany(actions.pickedCount),
      icon: <ArrowRightLeft size={13} />,
      onSelect: actions.onTransfer,
    });
  } else if (workspace.moved_to) {
    items.push("separator", { label: transfer.bringBack, icon: <Undo2 size={13} />, onSelect: actions.onTransfer });
  } else {
    const unavailable = transferUnavailable(workspace, french);
    items.push("separator", {
      label: transfer.transferItem,
      icon: <ArrowRightLeft size={13} />,
      disabled: unavailable != null,
      hint: unavailable ?? undefined,
      onSelect: actions.onTransfer,
    });
    if (here) {
      items.push({
        label: devServicesCopy(french).menu,
        icon: <Container size={13} />,
        onSelect: actions.onServices,
      });
    }
  }
  if (actions.archivable) {
    items.push("separator", { label: "Archive", icon: <Archive size={13} />, onSelect: actions.onArchive });
  }
  return items;
}

/** The sections a workspace can be filed in, "No section", and a new one. */
function sectionSubmenu(workspace: WorkspaceInfo, sections: SectionInfo[], onNewSection: () => void): MenuItem[] {
  const current = sections.some((s) => s.id === workspace.section_id) ? workspace.section_id : null;
  const items: MenuItem[] = sections.map((section) => ({
    label: section.name,
    disabled: section.id === current,
    hint: section.id === current ? "It is in this section" : undefined,
    onSelect: () => void setWorkspaceSection(workspace.id, section.id),
  }));
  items.push({
    label: "No section",
    disabled: current == null,
    hint: current == null ? "It is in no section" : "Back to Workspaces",
    onSelect: () => void setWorkspaceSection(workspace.id, null),
  });
  items.push("separator", { label: "New section…", icon: <FolderPlus size={13} />, onSelect: onNewSection });
  return items;
}

/** The apps the workspace folder opens in, read once and kept. */
let openApps: OpenApp[] | null = null;

function loadOpenApps() {
  void client
    .call("open.apps", {})
    .then((result) => (openApps = result.apps))
    .catch(() => {});
}

function openSubmenu(workspace: WorkspaceInfo): MenuItem[] {
  const open = async (app: string) => {
    try {
      await client.call("workspace.open", { workspace: workspace.id, app });
    } catch (error) {
      toast(`Could not open: ${errorMessage(error)}`, "error");
    }
  };
  if (!openApps) {
    loadOpenApps();
    return [{ label: "Looking for apps…", icon: <FolderOpen size={13} /> }];
  }
  return openApps.map((app) => ({ label: app.name, icon: <AppIcon kind={app.kind} size={13} />, onSelect: () => void open(app.id) }));
}

async function copy(text: string, what: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast(`${what} copied`);
  } catch {
    toast(`Could not copy the ${what.toLowerCase()}`, "error");
  }
}

/** The name, edited in place: Enter saves, Escape or leaving cancels. */
function RenameField({ workspace, onDone }: { workspace: WorkspaceInfo; onDone: () => void }) {
  const [value, setValue] = useState(workspaceName(workspace));
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  const save = async () => {
    onDone();
    if (value.trim() === workspaceName(workspace)) return;
    try {
      upsertWorkspace(await client.call("workspace.rename", { workspace: workspace.id, title: value }));
    } catch (error) {
      toast(`Could not rename: ${errorMessage(error)}`, "error");
    }
  };
  return (
    <input
      ref={input}
      value={value}
      onChange={(event) => setValue(event.target.value)}
      onClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Enter") void save();
        if (event.key === "Escape") onDone();
      }}
      onBlur={onDone}
      className="min-w-0 flex-1 rounded border border-line-strong bg-bg px-1 text-[13px] text-fg outline-none"
      data-testid="rename-field"
    />
  );
}

/** The tasks its agents run now (background commands, subagents), from the daemon. */
function TasksCount({ count }: { count: number }) {
  if (count <= 0) return null;
  const tip =
    count === 1
      ? "1 agent task running: a background command or a subagent"
      : `${count} agent tasks running: background commands or subagents`;
  return (
    <span title={tip} aria-label={tip} data-testid="row-tasks" className="inline-flex shrink-0 items-center gap-0.5 text-[10.5px] tabular-nums text-muted">
      <Layers size={10} />
      {count}
    </span>
  );
}

/**
 * A shared workspace (story 53): who looks at it, and the guests' suggestions waiting for the
 * user, in its chats.
 */
function ShareMark({ workspaceId }: { workspaceId: string }) {
  const indicator = shareIndicator(useWorkspaceShares((s) => s.shares[workspaceId]));
  const chatIds = useApp(useShallow((s) => chatIdsOf(s.chats, workspaceId)));
  const pending = useWorkspaceShares((s) => pendingCount(s.suggestions, chatIds));
  return (
    <>
      {pending > 0 && (
        <Badge tone="attention" title={`${pending} ${pending === 1 ? "suggestion waits" : "suggestions wait"} for you`}>
          {pending}
        </Badge>
      )}
      {indicator && (
        <span title={indicator.title} aria-label={indicator.title} data-testid="row-shared" className="inline-flex shrink-0 items-center gap-0.5 text-[10.5px] text-accent">
          <Users size={11} />
          {indicator.viewers > 0 && <span className="tabular-nums">{indicator.viewers}</span>}
        </span>
      )}
    </>
  );
}

function RowBadge({ workspace }: { workspace: WorkspaceInfo }) {
  if (workspace.moved_to) {
    const copy = transferCopy(navigator.language.startsWith("fr"));
    const idle = workspace.idle_since && copy.idleTitle(workspace.moved_to, clockTime(workspace.idle_since));
    return (
      <>
        {idle && (
          <span title={idle} aria-label={idle} data-testid="row-idle" className="inline-flex shrink-0 text-attention">
            <AlarmClock size={11} />
          </span>
        )}
        <Badge tone="accent" title={copy.movedBadgeTitle(workspace.moved_to)}>
          {copy.movedBadge(workspace.moved_to)}
        </Badge>
      </>
    );
  }
  if (workspace.state === "creating")
    return (
      <Badge tone="working">
        <Spinner size={10} /> creating
      </Badge>
    );
  if (workspace.state === "archiving")
    return (
      <Badge>
        <Spinner size={10} /> archiving
      </Badge>
    );
  if (workspace.state === "cleanup_failed") return <Badge tone="danger">cleanup failed</Badge>;
  if (workspace.state !== "ready") return null;
  if (workspace.activity === "needs_input") return <Badge tone="attention">needs input</Badge>;
  if (workspace.activity === "error") return <Badge tone="danger">error</Badge>;
  if (workspace.activity === "working")
    return (
      <span title="Working" className="text-working">
        <Spinner size={11} />
      </span>
    );
  return null;
}
