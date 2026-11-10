// The workspace's views (Chat, Diff, Files, Terminal, Processes): the tab bar and the view area,
// split as in VS Code into the main pane and at most one side pane, on the right or at the bottom.
// A tab goes to the side from its right-click menu, by dragging it to the area's right or bottom
// edge, or with ⌘\ (the last side view, Terminal at first). Every view keeps one place in the React
// tree whatever pane shows it, only its box moves: a terminal moved between panes keeps its xterm
// instance, its shell and its scrollback. While the main pane shows a design chat (story 47), the
// side pane waits (it does not open over the canvas) and ⌘\ hides or shows the design chat's chat
// column instead. ⌥⌘1–5 show a view as clicking its tab does (focused in the side pane when that one
// shows it), ⌃` the Terminal and from there the Chat.

import type { WorkspaceInfo } from "@chauffeur/protocol";
import { Columns2, PanelBottom, PanelRight, PanelTop, Rows2, X } from "lucide-react";
import { type CSSProperties, lazy, type ReactNode, Suspense, useEffect, useLayoutEffect, useRef, useState } from "react";
import { chatsIndicator, openChats } from "@/lib/chatTabs";
import { hasMod, kbd } from "@/lib/keys";
import { setMenuKeys } from "@/lib/native";
import { labelOf, pressed, viewKey, viewShortcut } from "@/lib/shortcuts";
import {
  type Box,
  dropZone,
  inBox,
  otherPosition,
  panes,
  shareAt,
  splitBoxes,
  type SplitPosition,
  tooSmall,
  type ViewId,
} from "@/lib/splitLayout";
import { routineOfWorkspace } from "@/lib/routines";
import { terminalsIndicator } from "@/lib/terminalActivity";
import { type ViewBadge, viewBadge } from "@/lib/viewBadge";
import { chatsOf, designChatShown, loadChats, mainView, setTab, splitOf, toast, toggleDesignChat, useApp } from "@/state/app";
import { useWorkspaceSelector, type WorkspaceSession } from "@/state/session";
import { closeSplit, moveToMainArea, openOnSide, placeSplit, resizeSplit, setSplitNarrow, toggleSplit } from "@/state/split";
import { ChatPanel } from "./chat/ChatPanel";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { DiffPanel } from "./DiffPanel";
import { FilesPanel } from "./FilesPanel";
import { ProcessesPanel } from "./ProcessesPanel";
import { RoutinePanel } from "./RoutinePanel";
import { cx, IconButton, Spinner } from "./ui";

import { WORKSPACE_EXTENSION_VIEWS, WorkspaceExtensionViews } from "@/extensions";
import { availableLayout, extensionEnabled } from "@/lib/extensions";

// xterm.js loads with the first terminal shown, not with the app.
const TerminalPanel = lazy(() => import("./TerminalPanel").then((module) => ({ default: module.TerminalPanel })));

const TABS: Array<{ id: ViewId; label: string }> = [
  { id: "chat", label: "Chat" },
  { id: "diff", label: "Diff" },
  { id: "files", label: "Files" },
  { id: "terminal", label: "Terminal" },
  { id: "processes", label: "Processes" },
  ...WORKSPACE_EXTENSION_VIEWS.map(({ id, label }) => ({ id, label })),
  // A routine's workspace only.
  { id: "routine", label: "Routine" },
];
const LABEL = Object.fromEntries(TABS.map((t) => [t.id, t.label])) as Record<ViewId, string>;

type Pane = "main" | "side" | "hidden";
/** A view tab being dragged: where the pointer is and where the tab would land. */
type Drag = { view: ViewId; x: number; y: number; target: SplitPosition | "main" | null };

const TOO_SMALL = "The window is too small for a split: it shows once wider.";
const DESIGN_WAITS = "The split does not open over a design chat's canvas: it shows on another chat or view.";

function boxOf(rect: DOMRect): Box {
  return { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
}

function boxStyle(box: Box): CSSProperties {
  return { left: box.left, top: box.top, width: box.width, height: box.height };
}

/** Puts the keyboard in a pane: its terminal, else its first field, else the pane itself. */
function focusPane(pane: HTMLElement) {
  const shown = (element: Element) => !element.parentElement?.closest(".invisible");
  const target =
    [...pane.querySelectorAll<HTMLElement>("textarea.xterm-helper-textarea")].find(shown) ??
    [...pane.querySelectorAll<HTMLElement>("textarea, input:not([type=hidden]), .cm-content")].find(shown) ??
    pane;
  target.focus({ preventScroll: true });
}

export function WorkspaceViews({
  workspace,
  session,
  setup,
}: {
  workspace: WorkspaceInfo;
  session: WorkspaceSession;
  setup: ReactNode;
}) {
  const id = workspace.id;
  const extensions = useApp((state) => state.extensions);
  const savedLayout = useApp((s) => splitOf(s, id));
  const layout = availableLayout(extensions, savedLayout);
  const routine = useApp((s) => routineOfWorkspace(s.routines, id));
  const savedMain = useApp((s) => mainView(s, id));
  // A routine deleted while its view was shown: back to the chat.
  const main = savedMain === "routine" && !routine ? "chat" : savedMain;
  const focusRequest = useApp((s) => s.splitFocus[id] ?? 0);
  const designShown = useApp((s) => designChatShown(s, id) != null);
  const chats = useApp((s) => openChats(chatsOf(s.chats, id)).length);
  const chatActivity = useApp((s) => chatsIndicator(openChats(chatsOf(s.chats, id))));
  const terminals = useWorkspaceSelector(session, (s) => s.terminals);
  const processes = useWorkspaceSelector(session, (s) => Object.values(s.processes).filter((p) => p.running).length);
  const badges: Partial<Record<ViewId, { badge: ViewBadge; title: string }>> = {
    chat: { badge: viewBadge(chats, chatActivity), title: plural(chats, "chat", "open") },
    terminal: {
      badge: viewBadge(Object.keys(terminals).length, terminalsIndicator(terminals)),
      title: plural(Object.keys(terminals).length, "terminal", "open"),
    },
    processes: { badge: viewBadge(processes), title: plural(processes, "process", "running") },
  };
  // The Chat tab counts the open chats before the Chat view first shows (it lists them too).
  useEffect(() => loadChats(id), [id]);
  const area = useRef<HTMLDivElement>(null);
  const tabBar = useRef<HTMLDivElement>(null);

  // The view area's size, read once per frame at most.
  const [size, setSize] = useState<{ width: number; height: number } | null>(null);
  useLayoutEffect(() => {
    const element = area.current;
    if (!element) return;
    let frame = 0;
    const measure = () => {
      frame = 0;
      const next = { width: element.clientWidth, height: element.clientHeight };
      setSize((prev) => (prev && prev.width === next.width && prev.height === next.height ? prev : next));
    };
    measure();
    const observer = new ResizeObserver(() => {
      if (!frame) frame = requestAnimationFrame(measure);
    });
    observer.observe(element);
    return () => {
      observer.disconnect();
      if (frame) cancelAnimationFrame(frame);
    };
  }, []);

  const narrow = size != null && tooSmall(layout.position, size.width, size.height);
  // The split waits on a design chat as in a small window: its view, asked for, shows in the main pane.
  const waiting = narrow || designShown;
  useEffect(() => setSplitNarrow(id, waiting), [id, waiting]);
  const side = panes(layout, main, waiting || size == null).side;
  const boxes = side && size ? splitBoxes(layout.position, layout.size[layout.position], size.width, size.height) : null;
  const place = (view: ViewId): Pane => (view === main ? "main" : view === side ? "side" : "hidden");

  // Terminals keep their xterm state while another view is shown; the Files view its open files
  // and unsaved edits.
  const [terminalMounted, setTerminalMounted] = useState(place("terminal") !== "hidden");
  const [filesMounted, setFilesMounted] = useState(place("files") !== "hidden");
  useEffect(() => {
    if (main === "terminal" || side === "terminal") setTerminalMounted(true);
    if (main === "files" || side === "files") setFilesMounted(true);
  }, [main, side]);

  // The pane last clicked or focused; the side pane's tab focuses it.
  const [focused, setFocused] = useState<"main" | "side">("main");
  const [flash, setFlash] = useState(0);
  useEffect(() => {
    if (!side) setFocused("main");
  }, [side]);
  useEffect(() => {
    if (!focusRequest) return;
    const pane = area.current?.querySelector<HTMLElement>('[data-pane="side"]');
    if (!pane) return;
    setFocused("side");
    setFlash((n) => n + 1);
    focusPane(pane);
  }, [focusRequest]);

  // ⌘\ opens or closes the side pane. Outside macOS, Ctrl+\ in the terminal stays the shell's.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!pressed(event, "view.split")) return;
      event.preventDefault();
      if (designChatShown(useApp.getState(), id)) toggleDesignChat();
      else if (toggleSplit(id) && useApp.getState().splitNarrow[id]) toast(TOO_SMALL);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [id]);

  const open = (view: ViewId, position: SplitPosition) => {
    openOnSide(id, view, position);
    if (designChatShown(useApp.getState(), id)) toast(DESIGN_WAITS);
    else if (size && tooSmall(position, size.width, size.height)) toast(TOO_SMALL);
  };

  // --- a view tab dragged to an edge of the view area, or back onto the tab bar ---
  const [drag, setDrag] = useState<Drag | null>(null);
  const suppressClick = useRef(false);
  const startDrag = (view: ViewId, event: React.PointerEvent) => {
    if (event.button !== 0) return;
    const start = { x: event.clientX, y: event.clientY };
    let started = false;
    const targetAt = (x: number, y: number): Drag["target"] => {
      const bar = tabBar.current?.getBoundingClientRect();
      if (bar && inBox(boxOf(bar), { x, y })) return "main";
      const rect = area.current?.getBoundingClientRect();
      return rect ? dropZone(boxOf(rect), { x, y }) : null;
    };
    const move = (e: PointerEvent) => {
      if (!started && Math.hypot(e.clientX - start.x, e.clientY - start.y) < 6) return;
      started = true;
      setDrag({ view, x: e.clientX, y: e.clientY, target: targetAt(e.clientX, e.clientY) });
    };
    const end = (e: PointerEvent | null) => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", key, true);
      if (!started) return;
      setDrag(null);
      // The click that follows the release is not a click on the tab.
      suppressClick.current = true;
      setTimeout(() => (suppressClick.current = false), 0);
      if (!e) return;
      const target = targetAt(e.clientX, e.clientY);
      if (target === "main") moveToMainArea(id, view);
      else if (target) open(view, target);
    };
    const up = (e: PointerEvent) => end(e);
    const cancel = () => end(null);
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !started) return;
      e.preventDefault();
      e.stopPropagation();
      end(null);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", key, true);
  };

  // --- the divider ---
  const [resizing, setResizing] = useState(false);
  const shareFor = (event: React.PointerEvent) => {
    const rect = area.current?.getBoundingClientRect();
    return rect ? shareAt(layout.position, boxOf(rect), { x: event.clientX, y: event.clientY }) : null;
  };

  // --- the tabs' right-click menu ---
  const [menu, setMenu] = useState<{ view: ViewId; x: number; y: number } | null>(null);
  const menuItems = (view: ViewId): MenuItem[] => {
    if (view === side) {
      const other = otherPosition(layout.position);
      return [
        other === "right"
          ? { label: "Move to the right", icon: <Columns2 size={13} />, shortcut: "r", onSelect: () => open(view, "right") }
          : { label: "Move below", icon: <Rows2 size={13} />, shortcut: "b", onSelect: () => open(view, "bottom") },
        { label: "Move back to the main area", icon: <PanelTop size={13} />, shortcut: "m", onSelect: () => moveToMainArea(id, view) },
      ];
    }
    return [
      { label: "Open on the right", icon: <Columns2 size={13} />, shortcut: "r", onSelect: () => open(view, "right") },
      { label: "Open below", icon: <Rows2 size={13} />, shortcut: "b", onSelect: () => open(view, "bottom") },
    ];
  };

  const slot = (view: ViewId, children: ReactNode) => {
    const pane = place(view);
    const box = pane === "side" ? boxes?.side : boxes?.main;
    return (
      <div
        key={view}
        data-view={view}
        data-pane={pane}
        tabIndex={-1}
        className={cx("absolute outline-none", !box && "inset-0", pane === "hidden" && "invisible")}
        style={box ? boxStyle(box) : undefined}
        onPointerDownCapture={() => pane !== "hidden" && setFocused(pane)}
        onFocusCapture={() => pane !== "hidden" && setFocused(pane)}
        onKeyDown={(event) => {
          // Keys the chat takes from the whole window (Esc interrupts its agent, Home and End
          // scroll it, ⌘F and ⌘G find in it) stay in the pane they were typed in.
          if (view === "chat") return;
          const vertical = event.key === "ArrowUp" || event.key === "ArrowDown";
          const find = hasMod(event) && !event.shiftKey && (event.key.toLowerCase() === "f" || event.key.toLowerCase() === "g");
          if (event.key === "Escape" || event.key === "Home" || event.key === "End" || (vertical && hasMod(event)) || find) {
            event.stopPropagation();
          }
        }}
      >
        {children}
      </div>
    );
  };

  const shown = (view: ViewId) => place(view) !== "hidden";
  // The view whose tabs Ctrl+Tab and the like go through: the focused pane's.
  const inUse = focused === "side" && side ? side : main;

  // Taken before the panes: the terminal, the editors and the composer's model favorites (Ctrl+Alt+1–9
  // outside macOS too) leave these keys to the views.
  const viewInUse = useRef(inUse);
  viewInUse.current = inUse;
  // The macOS menu's New and Close items act on the terminals while their view is the one in use.
  const terminalInUse = inUse === "terminal";
  useEffect(() => {
    setMenuKeys({ terminal: terminalInUse });
    return () => setMenuKeys({ terminal: false });
  }, [terminalInUse]);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const view = viewKey(event, viewInUse.current);
      if (!view || document.querySelector("[role=dialog]")) return;
      event.preventDefault();
      event.stopPropagation();
      // Never left typing into a view now hidden (its terminal).
      if (document.activeElement instanceof HTMLElement && area.current?.contains(document.activeElement)) document.activeElement.blur();
      setTab(id, view);
      // The keys go on to the view shown: its terminal or editor, else the view itself. A view shown
      // for the first time (Files, Terminal) mounts a frame later.
      const focusView = (again: boolean) => {
        const slot = area.current?.querySelector<HTMLElement>(`[data-view="${view}"]`);
        if (!slot) return again && requestAnimationFrame(() => focusView(false));
        if (view === "diff" || view === "processes") slot.focus({ preventScroll: true });
        else focusPane(slot);
      };
      requestAnimationFrame(() => focusView(true));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [id]);
  const preview =
    drag && size && (drag.target === "right" || drag.target === "bottom")
      ? splitBoxes(drag.target, layout.size[drag.target], size.width, size.height)
      : null;

  return (
    <>
      <div
        ref={tabBar}
        className={cx(
          "flex h-9 shrink-0 items-center gap-1 border-b border-line px-3 transition-colors",
          drag?.target === "main" && "bg-accent/10",
        )}
        role="tablist"
      >
        {TABS.filter((tab) => {
          if (tab.id === "routine") return routine != null;
          const contribution = WORKSPACE_EXTENSION_VIEWS.find((view) => view.id === tab.id);
          return !contribution || extensionEnabled(extensions, contribution.extension);
        }).map((t) => {
          const onSide = t.id === side;
          const badge = badges[t.id];
          return (
            <button
              key={t.id}
              role="tab"
              aria-selected={main === t.id}
              data-view-tab={t.id}
              data-side={onSide ? layout.position : undefined}
              title={`${onSide ? `Shown ${layout.position === "right" ? "on the right" : "below"}: click to focus it` : t.label} (${labelOf(viewShortcut(t.id))})${
                t.id === "chat" || t.id === "terminal" ? ` · ${labelOf("view.toggle-terminal")}: Chat or Terminal` : ""
              }`}
              onClick={() => {
                if (!suppressClick.current) setTab(id, t.id);
              }}
              onPointerDown={(event) => startDrag(t.id, event)}
              onContextMenu={(event) => {
                event.preventDefault();
                setMenu({ view: t.id, x: event.clientX, y: event.clientY });
              }}
              className={cx(
                "relative flex h-9 items-center gap-1 px-2.5 text-[12.5px]",
                main === t.id || (onSide && focused === "side") ? "text-fg" : "text-muted hover:text-fg",
                drag?.view === t.id && "opacity-50",
              )}
            >
              {t.label}
              {badge && <ViewTabBadge view={t.id} {...badge} />}
              {onSide &&
                (layout.position === "right" ? (
                  <Columns2 size={11} className="text-faint" aria-label="on the right" />
                ) : (
                  <Rows2 size={11} className="text-faint" aria-label="below" />
                ))}
              {main === t.id && (
                <span className={cx("absolute inset-x-2 -bottom-px h-px", side && focused === "side" ? "bg-line-strong" : "bg-accent")} />
              )}
            </button>
          );
        })}
        {drag?.target === "main" && (
          <span className="ml-auto text-[11.5px] text-accent">Move {LABEL[drag.view]} to the main area</span>
        )}
      </div>
      <div ref={area} className="relative min-h-0 flex-1" data-testid="view-area" data-split={side ? layout.position : undefined}>
        {shown("chat") &&
          slot(
            "chat",
            <ChatPanel workspace={workspace} session={session} setup={setup} inUse={inUse === "chat"} terminalInUse={terminalInUse} />,
          )}
        {shown("diff") && slot("diff", <DiffPanel workspace={workspace} session={session} />)}
        <WorkspaceExtensionViews extensions={extensions} workspace={workspace} main={main} side={side} slot={slot} />
        {shown("processes") && slot("processes", <ProcessesPanel workspace={workspace} session={session} />)}
        {routine && shown("routine") && slot("routine", <RoutinePanel workspace={workspace} routine={routine} />)}
        {filesMounted && workspace.state !== "creating" && slot("files", <FilesPanel workspace={workspace} session={session} visible={shown("files")} />)}
        {terminalMounted &&
          slot(
            "terminal",
            <Suspense fallback={null}>
              <TerminalPanel workspace={workspace} session={session} visible={shown("terminal")} inUse={inUse === "terminal"} />
            </Suspense>,
          )}

        {side && boxes && (
          <>
            <div
              role="separator"
              aria-orientation={layout.position === "right" ? "vertical" : "horizontal"}
              title="Drag to resize"
              className={cx("group absolute z-10 touch-none", layout.position === "right" ? "cursor-col-resize" : "cursor-row-resize")}
              style={boxStyle(boxes.divider)}
              onPointerDown={(event) => {
                if (event.button !== 0) return;
                event.preventDefault();
                event.currentTarget.setPointerCapture(event.pointerId);
                setResizing(true);
              }}
              onPointerMove={(event) => {
                if (!resizing) return;
                const share = shareFor(event);
                if (share != null) resizeSplit(id, share, false);
              }}
              onPointerUp={(event) => {
                if (!resizing) return;
                setResizing(false);
                const share = shareFor(event);
                if (share != null) resizeSplit(id, share, true);
              }}
              onPointerCancel={() => setResizing(false)}
              data-testid="split-divider"
            >
              <div
                className={cx(
                  "absolute transition-colors",
                  layout.position === "right" ? "inset-y-0 left-1/2 w-px -translate-x-1/2" : "inset-x-0 top-1/2 h-px -translate-y-1/2",
                  resizing ? "bg-accent" : "bg-line group-hover:bg-accent/60",
                )}
              />
            </div>
            <SideHeader
              view={side}
              position={layout.position}
              focused={focused === "side"}
              flash={flash}
              style={boxStyle(boxes.header)}
              onSwap={() => placeSplit(id, otherPosition(layout.position))}
              onClose={() => closeSplit(id)}
              onPointerDown={(event) => {
                setFocused("side");
                if (!(event.target as Element).closest("button")) startDrag(side, event);
              }}
            />
          </>
        )}

        {drag && preview && (
          <div
            className="pointer-events-none absolute z-20 flex items-center justify-center rounded-md border-2 border-accent/70 bg-accent/15"
            style={boxStyle({
              left: preview.header.left,
              top: preview.header.top,
              width: preview.header.width,
              height: preview.header.height + preview.side.height,
            })}
            data-testid="drop-zone"
            data-zone={drag.target ?? undefined}
          >
            <span className="rounded bg-raised px-2 py-1 text-[12px] text-fg shadow">
              {drag.target === "right" ? "Open on the right" : "Open below"}
            </span>
          </div>
        )}
        {resizing && <div className={cx("fixed inset-0 z-[65]", layout.position === "right" ? "cursor-col-resize" : "cursor-row-resize")} />}
      </div>

      {drag && (
        <>
          {/* Keeps the panes (a terminal, an editor) from reacting to the pointer meanwhile. */}
          <div className="fixed inset-0 z-[65] cursor-grabbing" />
          <div
            className="pointer-events-none fixed z-[66] rounded-md border border-line-strong bg-raised px-2 py-0.5 text-[12px] text-fg shadow-xl"
            style={{ left: drag.x + 12, top: drag.y + 10 }}
          >
            {LABEL[drag.view]}
          </div>
        </>
      )}

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.view)} onClose={() => setMenu(null)} />}
    </>
  );
}

function plural(n: number, noun: string, state: string): string {
  return `${n} ${n === 1 ? noun : noun === "process" ? "processes" : `${noun}s`} ${state}`;
}

/** After a view tab's label: what is open in the view, faint, and a terminal's activity in colour. */
function ViewTabBadge({ view, badge, title }: { view: ViewId; badge: ViewBadge; title: string }) {
  if (!badge.count && !badge.dot) return null;
  return (
    <span className="flex items-center gap-1" title={title} data-testid={`view-badge-${view}`}>
      {badge.count != null && <span className="text-[10.5px] tabular-nums text-faint">{badge.count}</span>}
      {badge.dot === "faint" && <span className="h-1 w-1 rounded-full bg-faint/70" />}
      {badge.dot === "unseen" && (
        <span title="A command finished" className="h-1.5 w-1.5 rounded-full bg-accent" data-testid="terminal-finished" />
      )}
      {badge.dot === "running" && (
        <span title="A command runs" className="h-1.5 w-1.5 animate-pulse-dot rounded-full bg-working" data-testid="terminal-running" />
      )}
      {badge.dot === "working" && (
        <span title="An agent is working" className="shrink-0 text-working" data-testid="chat-working">
          <Spinner size={10} />
        </span>
      )}
      {badge.dot === "attention" && (
        <span title="A chat needs input" className="h-1.5 w-1.5 rounded-full bg-attention" data-testid="chat-attention" />
      )}
    </span>
  );
}

function SideHeader({
  view,
  position,
  focused,
  flash,
  style,
  onSwap,
  onClose,
  onPointerDown,
}: {
  view: ViewId;
  position: SplitPosition;
  focused: boolean;
  /** Bumped to light the header up a moment, when its tab is clicked. */
  flash: number;
  style: CSSProperties;
  onSwap: () => void;
  onClose: () => void;
  onPointerDown: (event: React.PointerEvent) => void;
}) {
  const [lit, setLit] = useState(false);
  useEffect(() => {
    if (!flash) return;
    setLit(true);
    const timer = setTimeout(() => setLit(false), 450);
    return () => clearTimeout(timer);
  }, [flash]);
  return (
    <div
      className={cx(
        "absolute flex cursor-grab items-center gap-1 border-b border-line pl-3 pr-1 transition-colors duration-300 active:cursor-grabbing",
        lit && "bg-accent/15",
      )}
      style={style}
      onPointerDown={onPointerDown}
      data-testid="side-header"
      title="Drag onto the tab bar to move it back to the main area"
    >
      {focused && <span className="pointer-events-none absolute inset-x-0 top-0 h-px bg-accent" />}
      <span className={cx("text-[12px] font-medium", focused ? "text-fg" : "text-muted")}>{LABEL[view]}</span>
      <div className="ml-auto flex items-center">
        <IconButton
          title={position === "right" ? "Move below" : "Move to the right"}
          onClick={onSwap}
          className="h-6 w-6"
          data-testid="split-swap"
        >
          {position === "right" ? <PanelBottom size={13} /> : <PanelRight size={13} />}
        </IconButton>
        <IconButton title={`Close the split (${kbd("⌘\\")})`} onClick={onClose} className="h-6 w-6" data-testid="split-close">
          <X size={13} />
        </IconButton>
      </div>
    </div>
  );
}
