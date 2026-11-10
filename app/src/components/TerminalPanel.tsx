import type { OpenApp, TerminalInfo, WorkspaceInfo } from "@chauffeur/protocol";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { Copy, ListX, Pencil, Plus, SquareTerminal, X } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { base64ToBytes, workspaceRepos } from "@/lib/format";
import { stepTab } from "@/lib/keys";
import { onMenuAction } from "@/lib/native";
import { forApp, onceIn, terminalTabKey } from "@/lib/shortcuts";
import { errorMessage, isNotImplemented } from "@/lib/rpc";
import { type TerminalClose, closeToConfirm } from "@/lib/terminalClose";
import { shellSequence } from "@/lib/terminalKeys";
import { OutputSplicer } from "@/lib/terminalSplice";
import { type TerminalAction, terminalApp, terminalLabel, terminalMenu } from "@/lib/terminalTabs";
import { terminalFont } from "@/lib/fonts";
import { terminalTheme } from "@/lib/palette";
import { client, toast, useApp } from "@/state/app";
import { useWorkspaceSelector, type WorkspaceSession } from "@/state/session";
import { movedBefore } from "@/lib/tabDrag";
import { ConfirmTerminalClose } from "./ConfirmTerminalClose";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { Button, cx, EmptyState, IconButton, Select, Spinner } from "./ui";
import { useTabDrag } from "./useTabDrag";

/** The last `terminalRequest` run: nonces only grow, so a panel mounted again never runs one twice. */
let lastRequest = 0;

export function TerminalPanel({
  workspace,
  session,
  visible,
  inUse,
}: {
  workspace: WorkspaceInfo;
  session: WorkspaceSession;
  visible: boolean;
  /** Whether its pane is the one in use: the tab shortcuts are then its own. */
  inUse: boolean;
}) {
  const [terminals, setTerminals] = useState<TerminalInfo[] | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const [error, setError] = useState<{ message: string; notImplemented: boolean } | null>(null);
  const [repo, setRepo] = useState<string>("");
  const [opening, setOpening] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [menu, setMenu] = useState<TabMenu | null>(null);
  const [confirmClose, setConfirmClose] = useState<TerminalClose | null>(null);
  /** Bumped to put the keyboard back in the terminal shown, once a tab's name field is done. */
  const [refocus, setRefocus] = useState(0);
  const ready = workspace.state === "ready";
  const activity = useWorkspaceSelector(session, (s) => s.terminals);

  useEffect(() => {
    session.viewTerminal(visible ? active : null);
    return () => session.viewTerminal(null);
  }, [session, visible, active]);

  const strip = useRef<HTMLDivElement>(null);
  const tabDrag = useTabDrag({
    strip,
    ids: (terminals ?? []).map((t) => t.id),
    onMove: async (id, before) => {
      const shown = terminals ?? [];
      // Shown at once; the daemon's order replaces it, a refusal puts it back.
      const order = movedBefore(shown.map((t) => t.id), id, before);
      setTerminals(order.flatMap((tid) => shown.filter((t) => t.id === tid)));
      try {
        setTerminals((await client.call("terminal.move", { terminal: id, before })).terminals);
      } catch (err) {
        setTerminals(shown);
        toast(`Could not move the terminal tab: ${errorMessage(err)}`, "error");
      }
    },
  });

  const refresh = useCallback(async () => {
    try {
      const result = await client.call("terminal.list", { workspace: workspace.id });
      setTerminals(result.terminals);
      session.listTerminals(result.terminals);
      setError(null);
      setActive((current) =>
        current && result.terminals.some((t) => t.id === current) ? current : (result.terminals[0]?.id ?? null),
      );
    } catch (err) {
      setError({ message: errorMessage(err), notImplemented: isNotImplemented(err) });
      setTerminals([]);
    }
  }, [workspace.id, session]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // A login to redo in a terminal (an agent's, an MCP server's), asked from elsewhere in the app.
  const request = useApp((s) => s.terminalRequest[workspace.id]);
  useEffect(() => {
    if (!request || request.nonce <= lastRequest || !ready) return;
    lastRequest = request.nonce;
    void open(request.command);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- runs once per request
  }, [request, ready]);

  const open = async (command?: string) => {
    setOpening(true);
    try {
      const info = await client.call("terminal.open", {
        workspace: workspace.id,
        repo: command ? undefined : repo || undefined,
        cols: 100,
        rows: 30,
        command,
      });
      setTerminals((list) => [...(list ?? []), info]);
      session.terminalOpened(info.id);
      setActive(info.id);
    } catch (err) {
      toast(`Could not open a terminal: ${errorMessage(err)}`, "error");
    } finally {
      setOpening(false);
    }
  };

  const close = async (id: string) => {
    try {
      await client.call("terminal.close", { terminal: id });
    } catch (err) {
      toast(`Could not close the terminal: ${errorMessage(err)}`, "error");
    }
    setTerminals((list) => (list ?? []).filter((t) => t.id !== id));
    session.terminalClosed(id);
    // As in a browser, the tab after it shows, else the one before.
    const ids = (terminals ?? []).map((t) => t.id);
    const at = ids.indexOf(id);
    setActive((current) => (current === id ? (ids[at + 1] ?? ids[at - 1] ?? null) : current));
  };
  /** Closes the terminals `ids`, once the user said yes if a command runs in one of them. */
  const requestClose = (ids: string[]) => {
    const ask = closeToConfirm(terminals ?? [], ids, activity, terminalLabel);
    if (ask) setConfirmClose(ask);
    else void Promise.all(ids.map(close));
  };

  const step = useRef<(step: 1 | -1) => void>(() => {});
  step.current = (by) => {
    const target = stepTab((terminals ?? []).map((t) => t.id), active, by);
    if (target) setActive(target);
  };
  // ⌘T opens a terminal and ⌘W closes the one shown (Ctrl+Shift+T and Ctrl+Shift+W outside macOS),
  // from the keyboard or from macOS's menu (New Terminal, Close Terminal), whichever comes first.
  const tabAction = useRef<(action: "new" | "close") => void>(() => {});
  tabAction.current = (action) => {
    if (action === "close") {
      if (active) requestClose([active]);
    } else if (ready && !opening) void open();
  };
  useEffect(() => {
    if (!inUse) return;
    const run = (action: "new" | "close") => {
      if (document.querySelector("[role=dialog]")) return;
      onceIn(action === "new" ? "new-terminal" : "close-terminal", () => tabAction.current(action));
    };
    const onKey = (event: KeyboardEvent) => {
      const key = terminalTabKey(event);
      if (!key || key === "reopen" || document.querySelector("[role=dialog]")) return;
      event.preventDefault();
      if (key === "next" || key === "previous") step.current(key === "next" ? 1 : -1);
      // A held key opens or closes one terminal, not one per repeat.
      else if (!event.repeat) run(key);
    };
    window.addEventListener("keydown", onKey);
    const unlisten = onMenuAction((action) => {
      if (action === "new-terminal" || action === "close-terminal") run(action === "new-terminal" ? "new" : "close");
    });
    return () => {
      window.removeEventListener("keydown", onKey);
      unlisten();
    };
  }, [inUse]);

  // --- a tab's right-click menu, as chat tabs have ---
  const openMenu = (id: string, x: number, y: number) => {
    setMenu({ id, x, y, apps: null, cwd: null });
    const update = (part: Partial<TabMenu>) => setMenu((current) => (current?.id === id ? { ...current, ...part } : current));
    void client
      .call("open.apps", {})
      .then((result) => update({ apps: result.apps }))
      .catch(() => update({ apps: [] }));
    void cwdOf(id)
      .then((cwd) => update({ cwd }))
      .catch(() => {});
  };
  const cwdOf = async (id: string) => (await client.call("terminal.cwd", { terminal: id })).cwd;

  const finishRename = async (terminal: TerminalInfo, name: string | null) => {
    setRenaming(null);
    setRefocus((n) => n + 1);
    if (name === null) return;
    try {
      const info = await client.call("terminal.rename", { terminal: terminal.id, name });
      setTerminals((list) => (list ?? []).map((t) => (t.id === info.id ? info : t)));
    } catch (err) {
      toast(`Could not rename the terminal: ${errorMessage(err)}`, "error");
    }
  };

  const run = async (action: TerminalAction, id: string, apps: OpenApp[] | null, cwd: string | null) => {
    if (action === "rename") setRenaming(id);
    else if (action === "close") requestClose([id]);
    else if (action === "close-others") {
      setActive(id);
      requestClose((terminals ?? []).filter((t) => t.id !== id).map((t) => t.id));
    } else if (action === "open") {
      const app = apps ? terminalApp(apps) : null;
      if (!app) return;
      try {
        await client.call("workspace.open", { workspace: workspace.id, app: app.id, terminal: id });
      } catch (err) {
        toast(`Could not open the terminal's folder in ${app.name}: ${errorMessage(err)}`, "error");
      }
    } else {
      try {
        await navigator.clipboard.writeText(cwd ?? (await cwdOf(id)));
        toast("Path copied");
      } catch (err) {
        toast(`Could not copy the terminal's path: ${errorMessage(err)}`, "error");
      }
    }
  };

  const menuItems = ({ id, apps, cwd }: TabMenu): MenuItem[] => {
    const terminal = (terminals ?? []).find((t) => t.id === id);
    if (!terminal) return [];
    return terminalMenu({ terminal, terminals: terminals ?? [], apps, cwd }).map((item) =>
      item === "separator" ? item : { ...item, icon: MENU_ICONS[item.action], onSelect: () => void run(item.action, id, apps, cwd) },
    );
  };

  const repoOptions = [{ value: "", label: "Workspace folder" }, ...workspaceRepos(workspace).map((r) => ({ value: r, label: r }))];

  if (error && (terminals?.length ?? 0) === 0) {
    return (
      <EmptyState icon={<SquareTerminal size={22} />} title={error.notImplemented ? "Terminals are not available yet" : "Could not list terminals"}>
        {error.notImplemented ? "The daemon does not implement terminal.* yet." : error.message}
        <div className="mt-3">
          <Button size="sm" onClick={() => void refresh()}>
            Retry
          </Button>
        </div>
      </EmptyState>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex h-8 shrink-0 items-center gap-0.5 border-b border-line px-3">
        <div
          ref={strip}
          className="flex min-w-0 select-none items-center gap-0.5 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
          data-testid="terminal-tabs"
          onWheel={(event) => {
            // A mouse wheel only turns vertically: on the strip it scrolls sideways.
            if (event.deltaX === 0) event.currentTarget.scrollLeft += event.deltaY;
          }}
        >
          {(terminals ?? []).map((terminal, index) => (
            <div
              key={terminal.id}
              data-tab={terminal.id}
              style={tabDrag.tabStyle(terminal.id)}
              onPointerDown={renaming === terminal.id ? undefined : (event) => tabDrag.onPointerDown(terminal.id, event)}
              // The middle button closes a tab, as in a browser; on Linux it would paste or start autoscroll.
              onMouseDown={(event) => event.button === 1 && event.preventDefault()}
              onAuxClick={(event) => {
                if (event.button !== 1) return;
                event.preventDefault();
                requestClose([terminal.id]);
              }}
              onContextMenu={(event) => {
                event.preventDefault();
                openMenu(terminal.id, event.clientX, event.clientY);
              }}
              className={cx(
                "group flex h-6 shrink-0 items-center gap-1 rounded-md pl-2 pr-0.5 text-[12px]",
                terminal.id === active ? "bg-hover text-fg" : "text-muted hover:bg-hover/60",
              )}
            >
              {renaming === terminal.id ? (
                <TerminalRenameField terminal={terminal} label={terminalLabel(terminal, index)} onDone={(name) => finishRename(terminal, name)} />
              ) : (
                <button
                  onClick={() => {
                    if (!tabDrag.clickIsDrop()) setActive(terminal.id);
                  }}
                  onDoubleClick={() => setRenaming(terminal.id)}
                  className="flex items-center gap-1.5 whitespace-nowrap"
                >
                  <SquareTerminal size={11} className={terminal.running ? "text-success" : "text-faint"} />
                  {terminalLabel(terminal, index)}
                  {activity[terminal.id]?.unseen ? (
                    <span title="A command finished" className="h-1.5 w-1.5 rounded-full bg-accent" />
                  ) : (
                    activity[terminal.id]?.running && (
                      <span title="A command runs" className="h-1.5 w-1.5 animate-pulse-dot rounded-full bg-working" />
                    )
                  )}
                </button>
              )}
              <IconButton
                title="Close terminal"
                className="h-5 w-5 opacity-0 group-hover:opacity-100"
                onClick={() => {
                  if (!tabDrag.clickIsDrop()) requestClose([terminal.id]);
                }}
              >
                <X size={11} />
              </IconButton>
            </div>
          ))}
        </div>
        {ready && (
          <div className="ml-auto flex shrink-0 items-center gap-1.5">
            <Select value={repo} options={repoOptions} onChange={setRepo} title="Start in" />
            <Button size="sm" onClick={() => void open()} busy={opening}>
              <Plus size={12} /> Terminal
            </Button>
          </div>
        )}
      </div>
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu)} onClose={() => setMenu(null)} />}
      {confirmClose && (
        <ConfirmTerminalClose
          confirm={confirmClose}
          onCancel={() => setConfirmClose(null)}
          onConfirm={async () => {
            setConfirmClose(null);
            await Promise.all(confirmClose.ids.map(close));
          }}
        />
      )}
      <div className="relative min-h-0 flex-1 bg-[var(--terminal-bg)]">
        {terminals == null ? (
          <div className="flex h-full items-center justify-center text-faint">
            <Spinner />
          </div>
        ) : terminals.length === 0 ? (
          <EmptyState icon={<SquareTerminal size={22} />} title="No terminal">
            {ready ? "Open a terminal in the workspace folder or in one of its repos." : "This workspace is not ready."}
          </EmptyState>
        ) : (
          terminals.map((terminal) => (
            <div key={terminal.id} className={cx("absolute inset-0", terminal.id !== active && "invisible")}>
              <XtermView terminal={terminal} session={session} visible={visible && terminal.id === active} refocus={refocus} />
            </div>
          ))
        )}
      </div>
    </div>
  );
}

function XtermView({
  terminal,
  session,
  visible,
  refocus,
}: {
  terminal: TerminalInfo;
  session: WorkspaceSession;
  visible: boolean;
  refocus: number;
}) {
  const container = useRef<HTMLDivElement>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const termRef = useRef<Terminal | null>(null);
  /** Fits the terminal to its box and tells the daemon its new size. */
  const resizeRef = useRef<(() => void) | null>(null);
  const appearance = useApp((s) => s.appearance);
  const [exited, setExited] = useState<number | null | undefined>(terminal.running ? undefined : terminal.exit_code);

  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const term = new Terminal({
      ...terminalFont(useApp.getState().appearance),
      lineHeight: 1.15,
      cursorBlink: true,
      allowProposedApi: false,
      scrollback: 10000,
      theme: terminalTheme(useApp.getState().appearance?.palette),
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    // The keys that show another tab, and those the user gave a shortcut, are the app's: xterm
    // would send them to the shell first.
    term.attachCustomKeyEventHandler((event) => {
      if (forApp(event)) return false;
      const sequence = shellSequence(event);
      if (sequence === null) return true;
      event.preventDefault();
      term.input(sequence);
      return false;
    });
    term.open(element);
    termRef.current = term;
    fitRef.current = fit;

    // The scrollback first, then live output spliced after it by offset (no byte lost or
    // repeated). Missed output (the connection dropped) means a fresh snapshot.
    let cancelled = false;
    let loading = false;
    let reloadQueued = false;
    const splicer = new OutputSplicer(
      (bytes) => term.write(bytes),
      () => void load(true),
    );
    const load = async (again: boolean) => {
      if (loading) {
        reloadQueued = true;
        return;
      }
      loading = true;
      try {
        const snapshot = await client.call("terminal.snapshot", { terminal: terminal.id });
        if (cancelled) return;
        if (again) term.reset();
        term.write(base64ToBytes(snapshot.data));
        splicer.snapshotWritten(snapshot.end_offset);
        if (!snapshot.terminal.running) setExited(snapshot.terminal.exit_code);
      } catch (err) {
        if (cancelled) return;
        term.write(`\r\n[could not load the scrollback: ${errorMessage(err)}]\r\n`);
      } finally {
        loading = false;
        if (reloadQueued && !cancelled) {
          reloadQueued = false;
          void load(true);
        }
      }
    };
    const stop = session.onTerminal(terminal.id, {
      output: (bytes, offset) => splicer.push(offset, bytes),
      exit: (code) => setExited(code),
    });
    void load(false);

    const input = term.onData((data) => {
      void client.call("terminal.input", { terminal: terminal.id, data }).catch(() => {});
    });
    let lastSize = "";
    const resize = () => {
      if (!element.offsetWidth || !element.offsetHeight) return;
      try {
        fit.fit();
      } catch {
        return;
      }
      const size = `${term.cols}x${term.rows}`;
      if (size === lastSize) return;
      lastSize = size;
      void client.call("terminal.resize", { terminal: terminal.id, cols: term.cols, rows: term.rows }).catch(() => {});
    };
    resizeRef.current = resize;
    // Refitted on every change of its box (the window, the split's divider, a pane moved), once a
    // frame at most, so that lines wrap at the new width and the shell learns its new size.
    let frame = 0;
    const observer = new ResizeObserver(() => {
      if (!frame)
        frame = requestAnimationFrame(() => {
          frame = 0;
          resize();
        });
    });
    observer.observe(element);
    resize();
    return () => {
      cancelled = true;
      if (frame) cancelAnimationFrame(frame);
      observer.disconnect();
      input.dispose();
      stop();
      term.dispose();
      termRef.current = null;
      resizeRef.current = null;
    };
  }, [terminal.id, session]);

  // Another font or size changes how many columns and rows fit; another theme only the colours.
  useEffect(() => {
    const term = termRef.current;
    if (!term) return;
    const theme = terminalTheme(appearance?.palette);
    if (JSON.stringify(term.options.theme) !== JSON.stringify(theme)) term.options.theme = theme;
    const font = terminalFont(appearance);
    if (term.options.fontFamily === font.fontFamily && term.options.fontSize === font.fontSize) return;
    term.options.fontFamily = font.fontFamily;
    term.options.fontSize = font.fontSize;
    resizeRef.current?.();
  }, [appearance]);

  useEffect(() => {
    if (!visible) return;
    try {
      fitRef.current?.fit();
    } catch {
      // not laid out yet
    }
    termRef.current?.focus();
  }, [visible, refocus]);

  return (
    <div className="flex h-full flex-col">
      <div ref={container} className="min-h-0 flex-1" />
      {exited !== undefined && (
        <div className="shrink-0 border-t border-line px-3 py-1 text-[11.5px] text-faint">
          Process exited{exited != null ? ` with code ${exited}` : ""}.
        </div>
      )}
    </div>
  );
}

const MENU_ICONS: Record<TerminalAction, ReactNode> = {
  rename: <Pencil size={13} />,
  close: <X size={13} />,
  "close-others": <ListX size={13} />,
  open: <SquareTerminal size={13} />,
  "copy-path": <Copy size={13} />,
};

/** A tab's right-click menu: where it opened, and the apps and the shell's folder once read. */
type TabMenu = { id: string; x: number; y: number; apps: OpenApp[] | null; cwd: string | null };

/** A tab's name, edited in place: Enter saves (empty: its title), Escape or leaving cancels (null), as
 * does Enter on the name shown. */
function TerminalRenameField({ terminal, label, onDone }: { terminal: TerminalInfo; label: string; onDone: (name: string | null) => void }) {
  const [value, setValue] = useState(label);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  return (
    <input
      ref={input}
      value={value}
      onChange={(event) => setValue(event.target.value)}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Enter") onDone(value.trim() === label ? null : value.trim());
        if (event.key === "Escape") onDone(null);
      }}
      onBlur={() => onDone(null)}
      placeholder={terminal.title || "Its title"}
      className="w-[160px] min-w-0 rounded border border-line-strong bg-bg px-1 text-[12px] text-fg outline-none"
      data-testid="terminal-rename-field"
    />
  );
}
