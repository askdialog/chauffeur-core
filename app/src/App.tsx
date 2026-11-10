import { Plus, Telescope, X } from "lucide-react";
import { useEffect } from "react";
import { CreateWorkspaceDialog } from "./components/CreateWorkspaceDialog";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { GithubSetup } from "./components/GithubSetup";
import { QuickSwitcher } from "./components/QuickSwitcher";
import { SettingsDialog } from "./components/SettingsDialog";
import { FindRoutinesDialog } from "./components/FindRoutinesDialog";
import { TransferBatch } from "./components/TransferBatch";
import { ShowSidebarButton } from "./components/ShowSidebarButton";
import { Sidebar } from "./components/Sidebar";
import { Button, cx, EmptyState, Kbd } from "./components/ui";
import { WorkspaceView } from "./components/WorkspaceView";
import { onMenuAction, openMobileConnection } from "./lib/native";
import {
  client,
  dismissToast,
  goInHistory,
  goToWorkspace,
  openCreate,
  openSettings,
  openSwitcher,
  requestFiles,
  selectWorkspace,
  showOffice,
  startApp,
  toggleSidebar,
  useApp,
  zoomApp,
} from "./state/app";
import { digitOf, fromTerminal, hasMod, historyKey, kbd } from "./lib/keys";
import { type Id, pressed } from "./lib/shortcuts";
import { sendToastReport, type Toast, useToasts } from "./state/toasts";
import { sidebarGroups, visibleRows } from "./lib/workspaceOrder";

import { GlobalExtensionView } from "./extensions";

export function App() {
  useEffect(() => {
    startApp();
  }, []);
  useShortcuts();

  const workspace = useApp((s) => (s.selectedId ? s.workspaces[s.selectedId] : undefined));
  const loaded = useApp((s) => s.workspacesLoaded);
  const firstRun = useApp((s) => s.workspacesLoaded && Object.keys(s.workspaces).length === 0);
  const sidebarHidden = useApp((s) => s.sidebarHidden);
  const officeShown = useApp((s) => s.officeShown);
  // The Office is there only while it shows: closed, it lets go of its scene, models and textures,
  // so that it costs nothing to whoever does not look at it.

  return (
    <div className="flex h-full">
      {/* The 3D view has its own bar of workspaces, at its top. */}
      {!sidebarHidden && !officeShown && <Sidebar />}
      <main className="relative flex min-w-0 flex-1 flex-col">
        <ConnectionBanner />
        {officeShown && (
          <div className="relative min-h-0 flex-1" data-testid="office-view">
            <ErrorBoundary>
              <GlobalExtensionView id="office" />
            </ErrorBoundary>
          </div>
        )}
        {officeShown ? null : workspace ? (
          <ErrorBoundary key={workspace.id}>
            <WorkspaceView workspace={workspace} />
          </ErrorBoundary>
        ) : (
          <>
          <ShowSidebarButton className="absolute top-2.5 left-2.5" />
          <EmptyState title={!loaded ? "Loading…" : firstRun ? "Welcome to Chauffeur" : "No workspace selected"}>
            {loaded && (
              <div className="flex flex-col items-center gap-3">
                <GithubSetup />
                <span>A workspace is one worktree per repo on one branch, with its chats, terminals and scripts.</span>
                <div className="flex items-center gap-2">
                  <Button variant="primary" onClick={() => openCreate()}>
                    <Plus size={13} /> New workspace <Kbd>{kbd("⌘N")}</Kbd>
                  </Button>
                  <Button onClick={() => openCreate(true, true)} title="Ask about the code at each repo's base branch, read-only">
                    <Telescope size={13} /> Explore <Kbd>{kbd("⇧⌘N")}</Kbd>
                  </Button>
                </div>
              </div>
            )}
          </EmptyState>
          </>
        )}
      </main>
      <CreateWorkspaceDialog />
      <QuickSwitcher />
      <SettingsDialog />
      <FindRoutinesDialog />
      <TransferBatch />
      <Toasts />
    </div>
  );
}

function useShortcuts() {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const on = (id: Id) => pressed(event, id);
      if (on("workspace.new") || on("exploration.new")) {
        event.preventDefault();
        openCreate(true, on("exploration.new"));
      } else if (on("files.goto") || on("files.search")) {
        const id = useApp.getState().selectedId;
        if (id) {
          event.preventDefault();
          requestFiles(id, { kind: on("files.goto") ? "goto" : "search" });
        }
      } else if (on("settings.open")) {
        event.preventDefault();
        openSettings(!useApp.getState().settingsOpen);
      } else if (on("office.show")) {
        // On macOS the Window menu takes ⇧⌘O first; this is Linux's, which has no menu.
        event.preventDefault();
        showOffice();
      } else if (on("sidebar.toggle")) {
        event.preventDefault();
        toggleSidebar();
      } else if (on("zoom.in") || on("zoom.out") || on("zoom.reset")) {
        // A full-screen image takes + − 0 for itself before this.
        event.preventDefault();
        zoomApp(on("zoom.in") ? 1 : on("zoom.out") ? -1 : 0);
      } else if (historyKey(event) !== null && !fromTerminal(event)) {
        // Not when a text editor took ⌘[ for itself (indenting less); outside macOS the terminal
        // keeps Ctrl+[, its Escape.
        if (event.defaultPrevented) return;
        event.preventDefault();
        goInHistory(historyKey(event) === "back" ? -1 : 1);
      } else if (on("switcher.open")) {
        event.preventDefault();
        openSwitcher(!useApp.getState().switcherOpen);
      } else if (on("workspace.goto")) {
        // In the Office, its chat opens beside the scene.
        const { workspaces, sections, collapsedParents } = useApp.getState();
        const list = visibleRows(sidebarGroups(workspaces, sections), new Set(collapsedParents));
        const target = list[(digitOf(event) ?? 0) - 1];
        if (target) {
          event.preventDefault();
          goToWorkspace(target.workspace.id);
        }
      } else if (hasMod(event) && !event.shiftKey && !event.altKey && event.key.toLowerCase() === "w" && !fromTerminal(event)) {
        // ⌘W closes the chat shown (the chat panel's own shortcut, whatever its keys), never the window.
        event.preventDefault();
      }
    };
    window.addEventListener("keydown", onKey);
    // The web view's own menu (Reload, Inspect…) is of no use in the app: only text fields, the
    // terminal and selected text keep it, for copy and paste.
    const onContextMenu = (event: MouseEvent) => {
      const target = event.target as Element | null;
      if (target?.closest?.("input, textarea, [contenteditable=true], .xterm")) return;
      if (window.getSelection()?.toString()) return;
      event.preventDefault();
    };
    window.addEventListener("contextmenu", onContextMenu);
    const unlisten = onMenuAction((action) => {
      if (action === "new-workspace") openCreate();
      if (action === "new-exploration") openCreate(true, true);
      if (action === "switch-workspace") openSwitcher();
      if (action === "settings") openSettings();
      if (action === "mobile-connection") {
        void client.call("mobile.status", {}).then(status => {
          if (status.phase === "ready") return openMobileConnection();
          openSettings(true, "mobile");
        }).catch(() => openSettings(true, "mobile"));
      }
      if (action === "toggle-sidebar") toggleSidebar();
      if (action === "office") showOffice();
      if (action === "zoom-in") zoomApp(1);
      if (action === "zoom-out") zoomApp(-1);
      if (action === "zoom-reset") zoomApp(0);
    });
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("contextmenu", onContextMenu);
      unlisten();
    };
  }, []);

  // Drop a selection that no longer exists once the list is known.
  const selectedMissing = useApp((s) => s.workspacesLoaded && !!s.selectedId && !s.workspaces[s.selectedId]);
  useEffect(() => {
    if (selectedMissing) selectWorkspace(null);
  }, [selectedMissing]);
}

function ConnectionBanner() {
  const connection = useApp((s) => s.connection);
  if (connection.status === "open") return null;
  const message =
    connection.status === "closed"
      ? "Disconnected from chauffeurd."
      : connection.error
        ? `chauffeurd is not reachable (${connection.error}); retrying…`
        : "Connecting to chauffeurd…";
  return (
    <div
      className={cx(
        "shrink-0 border-b px-4 py-1.5 text-[12px]",
        connection.status === "connecting" && !connection.error
          ? "border-line text-muted"
          : "border-attention/30 bg-attention/10 text-attention",
      )}
      data-testid="connection-banner"
    >
      {message}
    </div>
  );
}

/** Under an error toast: its report offered (Send report, Always send), on its way, or done. */
function ToastReport({ toast: t }: { toast: Toast }) {
  if (t.report === "ask" || t.report === "failed") {
    return (
      <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11.5px]" data-testid="toast-report">
        {t.report === "failed" && <span className="w-full text-[#f7b4b4]/80">Not sent: {t.reportNote}</span>}
        <button
          onClick={() => void sendToastReport(t.id)}
          className="rounded border border-danger/40 px-1.5 py-0.5 text-fg hover:bg-danger/20"
          data-testid="toast-send-report"
        >
          {t.report === "failed" ? "Try again" : "Send report"}
        </button>
        <button
          onClick={() => void sendToastReport(t.id, true)}
          className="text-faint underline-offset-2 hover:text-fg hover:underline"
          title="Report every error from now on (Settings › General)"
          data-testid="toast-always-send"
        >
          Always send
        </button>
      </div>
    );
  }
  return (
    <div className="mt-1 text-[11.5px] text-faint" data-testid="toast-report">
      {t.report === "sending" ? "Sending the report…" : t.reportNote}
    </div>
  );
}

export function Toasts() {
  const toasts = useToasts((s) => s.toasts);
  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-[60] flex w-[380px] flex-col gap-2">
      {toasts.map((t) => (
        <div
          key={t.id}
          role="status"
          className={cx(
            "pointer-events-auto flex items-start gap-2 rounded-md border px-3 py-2 text-[12.5px] shadow-xl",
            t.level === "error" ? "border-danger/40 bg-[#2a1617] text-[#f7b4b4]" : "border-line-strong bg-raised text-fg",
          )}
        >
          <div className="min-w-0 flex-1">
            <span className="break-words selectable">{t.message}</span>
            {t.report && <ToastReport toast={t} />}
          </div>
          <button onClick={() => dismissToast(t.id)} className="text-faint hover:text-fg" aria-label="Dismiss">
            <X size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}
