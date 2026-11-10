// "Open in Cursor": the workspace folder in an editor, a terminal or Finder, in one click; in an
// editor, one repo's worktree too. ⌘O opens the workspace folder in the default app (the last
// editor or terminal used). While a design chat is shown (story 47), its design folder opens instead,
// never in a terminal, and Browser opens a screen of its canvas at its served URL. A workspace moved
// to a GCP worker opens there, in Cursor or VS Code over SSH: only those editors are offered; until
// it can (the Include line missing from the ssh config…), the button says why.

import type { OpenApp, OpenAppKind, WorkspaceInfo } from "@chauffeur/protocol";
import { AlertTriangle, Check, ChevronDown, Code, FolderOpen, Globe, SquareTerminal } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { browserUrl, designApp } from "@/lib/designHandoff";
import { openExternal } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { remoteEditors } from "@/lib/workers";
import { client, designChatShown, toast, useApp } from "@/state/app";
import { cx } from "./ui";
import { kbd } from "../lib/keys";
import { pressed } from "../lib/shortcuts";

const WIDTH = 250;

export function AppIcon({ kind, size = 12 }: { kind: OpenAppKind; size?: number }) {
  if (kind === "terminal") return <SquareTerminal size={size} />;
  if (kind === "files") return <FolderOpen size={size} />;
  return <Code size={size} />;
}

export function OpenButton({
  workspace,
  repos,
  remote,
  issue,
}: {
  workspace: WorkspaceInfo;
  repos: string[];
  /** The GCP worker the workspace moved to, where it opens over SSH. */
  remote?: string;
  /** Why it does not open there yet, with what to do. */
  issue?: string | null;
}) {
  const [found, setFound] = useState<OpenApp[] | null>(null);
  const apps = remote && found ? remoteEditors(found) : found;
  const [menu, setMenu] = useState(false);
  const anchor = useRef<HTMLDivElement>(null);

  const loadApps = () =>
    client
      .call("open.apps", {})
      .then((result) => setFound(result.apps))
      .catch(() => setFound([]));
  useEffect(() => {
    void loadApps();
  }, []);

  const design = useApp((s) => (remote ? null : designChatShown(s, workspace.id)));
  const screenUrl = useApp((s) => (design ? browserUrl(s.canvasPage[design] ?? [], s.canvasSelection[design] ?? []) : null));
  const app = design ? designApp(apps ?? []) : (apps?.find((a) => a.is_default) ?? apps?.[0] ?? null);
  const folder = design ? "the design folder" : remote ? `the workspace folder on ${remote}` : "the workspace folder";

  const open = async (repo?: string, appId = app?.id) => {
    setMenu(false);
    if (issue) {
      toast(issue, "error");
      return;
    }
    try {
      const result = await client.call(
        "workspace.open",
        design ? { workspace: workspace.id, app: appId, design_chat: design } : { workspace: workspace.id, repo, app: appId },
      );
      // An editor or terminal used becomes the default; Finder never does.
      if (result.app.is_default) {
        setFound((current) => current?.map((a) => ({ ...a, is_default: a.id === result.app.id })) ?? current);
      }
    } catch (error) {
      toast(`Could not open: ${errorMessage(error)}`, "error");
    }
  };

  // ⌘O, unless a dialog covers the workspace.
  const latest = useRef(open);
  latest.current = open;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!pressed(event, "open.app")) return;
      if (document.querySelector("[role=dialog], [data-popover]")) return;
      event.preventDefault();
      void latest.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // On a worker without Cursor or VS Code here, there is nothing to open it with.
  if (remote && apps?.length === 0) return null;
  if (issue) {
    return (
      <button
        type="button"
        onClick={() => void open()}
        title={issue}
        data-testid="open-workspace"
        className="inline-flex h-6 items-center gap-1.5 rounded-md border border-line bg-raised px-2 text-[12px] font-medium text-fg hover:bg-hover"
      >
        <AlertTriangle size={12} className="text-attention" />
        Open
      </button>
    );
  }
  return (
    <div ref={anchor} className="inline-flex h-6 items-stretch rounded-md border border-line bg-raised text-[12px] text-fg">
      <button
        type="button"
        onClick={() => void open()}
        title={`Open ${folder} in ${app?.name ?? "the default app"} (${kbd("⌘O")})`}
        data-testid="open-workspace"
        className="inline-flex items-center gap-1.5 rounded-l-md px-2 font-medium hover:bg-hover"
      >
        {app ? <AppIcon kind={app.kind} /> : <FolderOpen size={12} />}
        {app ? app.name : "Open"}
      </button>
      <button
        type="button"
        onClick={() => {
          if (!menu) void loadApps();
          setMenu((m) => !m);
        }}
        title={design ? "Open the design folder with another app, or a screen in the browser" : "Open with another app, or one repo in an editor"}
        data-testid="open-menu"
        className={cx("inline-flex items-center rounded-r-md border-l border-line px-1 hover:bg-hover", menu && "bg-hover")}
      >
        <ChevronDown size={12} className="text-faint" />
      </button>
      {menu && (
        <OpenMenu
          anchor={anchor}
          apps={design ? (apps ?? []).filter((a) => a.kind !== "terminal") : (apps ?? [])}
          app={app}
          repos={design ? [] : repos}
          browser={design ? { url: screenUrl } : null}
          onOpen={(appId, repo) => void open(repo, appId)}
          onBrowser={(url) => {
            setMenu(false);
            void openExternal(url).catch((error) => toast(`Could not open the browser: ${errorMessage(error)}`, "error"));
          }}
          onClose={() => setMenu(false)}
        />
      )}
    </div>
  );
}

function OpenMenu({
  anchor,
  apps,
  app,
  repos,
  browser,
  onOpen,
  onBrowser,
  onClose,
}: {
  anchor: React.RefObject<HTMLDivElement | null>;
  apps: OpenApp[];
  app: OpenApp | null;
  repos: string[];
  /** A design chat's: the screen Browser opens (the selected one, else the page's first), if any. */
  browser: { url: string | null } | null;
  /** The workspace folder in an app; one repo's worktree when `repo` is given. */
  onOpen: (appId: string, repo?: string) => void;
  onBrowser: (url: string) => void;
  onClose: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const [style, setStyle] = useState<React.CSSProperties>({ visibility: "hidden" });
  useLayoutEffect(() => {
    const rect = anchor.current?.getBoundingClientRect();
    if (!rect) return;
    setStyle({ top: rect.bottom + 6, left: Math.max(8, Math.min(rect.right - WIDTH, window.innerWidth - WIDTH - 8)), width: WIDTH });
  }, [anchor]);

  useEffect(() => {
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (panel.current?.contains(target) || anchor.current?.contains(target)) return;
      onClose();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    };
    document.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [anchor, onClose]);

  const groups: Array<{ kind: OpenAppKind; label: string }> = [
    { kind: "editor", label: "Editors" },
    { kind: "terminal", label: "Terminals" },
    { kind: "files", label: "Files" },
  ];

  return createPortal(
    <div
      ref={panel}
      data-popover
      className="fixed z-[60] overflow-hidden rounded-xl border border-line-strong bg-sidebar p-1.5 text-[13px] shadow-2xl shadow-black/70"
      style={style}
    >
      <div className="max-h-[60vh] overflow-y-auto rounded-lg border border-line bg-raised py-1">
        {groups.map(({ kind, label }) => {
          const list = apps.filter((a) => a.kind === kind);
          if (list.length === 0) return null;
          return (
            <div key={kind}>
              <Label>{label}</Label>
              {list.map((candidate) => (
                <div key={candidate.id} className="group">
                  <Row onClick={() => onOpen(candidate.id)} testId={`open-app-${candidate.id}`}>
                    <span className="text-faint">
                      <AppIcon kind={candidate.kind} size={13} />
                    </span>
                    <span className="text-fg">{candidate.name}</span>
                    {candidate.id === app?.id && (
                      <span className="ml-auto flex items-center gap-1 text-[11.5px] text-faint">
                        {kbd("⌘O")} <Check size={12} />
                      </span>
                    )}
                  </Row>
                  {/* One repo alone in an editor: its own git and TypeScript project. */}
                  {kind === "editor" && repos.length > 1 && (
                    <div className="mx-1 mb-0.5 hidden flex-wrap gap-1 pr-2 pl-8 group-hover:flex">
                      {repos.map((repo) => (
                        <button
                          key={repo}
                          type="button"
                          onMouseDown={(e) => e.preventDefault()}
                          onClick={() => onOpen(candidate.id, repo)}
                          title={`Open ${repo} alone in ${candidate.name}`}
                          className="rounded border border-line px-1.5 text-[11.5px] text-muted hover:border-line-strong hover:text-fg"
                          data-testid={`open-repo-${candidate.id}-${repo}`}
                        >
                          {repo}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              ))}
            </div>
          );
        })}
        {browser && (
          <div>
            <Label>Browser</Label>
            <Row
              onClick={() => browser.url && onBrowser(browser.url)}
              disabled={!browser.url}
              title={browser.url ? `Open ${browser.url}` : "No screen to open: this canvas page has none yet"}
              testId="open-browser"
            >
              <span className="text-faint">
                <Globe size={13} />
              </span>
              <span className="text-fg">Browser</span>
              {browser.url && (
                <span className="ml-auto min-w-0 truncate text-[11.5px] text-faint">{screenName(browser.url)}</span>
              )}
            </Row>
          </div>
        )}
      </div>
    </div>,
    document.body,
  );
}

/** The file a screen's URL serves, as the menu names it. */
function screenName(url: string): string {
  const last = url.split("?")[0].split("/").pop() ?? url;
  try {
    return decodeURIComponent(last);
  } catch {
    return last;
  }
}

function Label({ children }: { children: React.ReactNode }) {
  return <div className="px-3 pt-1.5 pb-1 text-[11px] font-medium text-faint">{children}</div>;
}

function Row({
  children,
  onClick,
  testId,
  disabled,
  title,
}: {
  children: React.ReactNode;
  onClick: () => void;
  testId?: string;
  disabled?: boolean;
  title?: string;
}) {
  return (
    <div
      role="button"
      aria-disabled={disabled || undefined}
      title={title}
      data-testid={testId}
      onMouseDown={(e) => e.preventDefault()}
      onClick={disabled ? undefined : onClick}
      className={cx("mx-1 flex h-8 items-center gap-2 rounded-md px-2.5", disabled ? "cursor-default opacity-50" : "hover:bg-hover")}
    >
      {children}
    </div>
  );
}
