import { expect, spyOn } from "bun:test";
import type { GithubStatus, WorkspaceInfo } from "@chauffeur/protocol";
import { domTest } from "@/test/dom";

const file = import.meta.path;
const status: GithubStatus = { git: true, git_install: "xcode", gh: "gh", own: false, version: "", download_size: null, account: "alice", organizations: [], organization: "", connecting: null };

domTest("local repositories make GitHub setup optional but missing git stays actionable", file, async ({ render, until }) => {
  const { client } = await import("@/state/app");
  const { GithubSetup } = await import("./GithubSetup");
  const { refreshGithub } = await import("@/state/github");
  const call = spyOn(client, "call").mockResolvedValue(status);
  try {
    await render(<GithubSetup compact localReposSelected />);
    await until(() => !!document.querySelector('[data-testid="github-optional"]'));
    expect(document.body.textContent).toContain("GitHub connected as alice");
    expect(document.querySelector('[data-testid="github-setup"]')).toBeNull();
    call.mockResolvedValue({ ...status, git: false });
    await refreshGithub();
    await until(() => !!document.querySelector('[data-testid="install-git"]'));
  } finally { call.mockRestore(); }
});

domTest("process configuration guides creation when missing and opens existing TOML in Files", file, async ({ render, click, until }) => {
  const { client, useApp } = await import("@/state/app");
  const { RpcCallError } = await import("@/lib/rpc");
  const { WorkspaceSession } = await import("@/state/session");
  const { ProcessesPanel } = await import("./ProcessesPanel");
  const workspace: WorkspaceInfo = {
    id: "test", slug: "test", title: null, path: "/tmp/test", branch: "test", parent_id: null,
    state: "ready", activity: "idle", explore: false, tickets: [], port_base: 0, port_count: 0,
    created_at: "2026-11-16T00:00:00Z", archived_at: null, last_error: null, unread: false,
    pinned: false, position: 0, running_tasks: 0, section_id: null, section: null,
    worktrees: [{ id: "tree", repo: "owner/shop", path: "/tmp/test/shop", branch: "test", base_commit: null, base_branch: "main", origin: "workspace", group: null, removed: false }],
  };
  let missing = true;
  const call = spyOn(client, "call").mockImplementation(async (method) => {
    if (method === "files.read" && missing) throw new RpcCallError(method, { code: 404, message: "missing" });
    if (method === "process.list") return { processes: [], scripts: [] };
    return { terminals: [] };
  });
  const session = new WorkspaceSession(workspace.id);
  try {
    await render(<ProcessesPanel workspace={workspace} session={session} />);
    await until(() => !!document.querySelector('[data-testid="no-run-scripts"]'));
    await click('[data-testid="no-run-scripts"] button');
    expect(call).toHaveBeenCalledWith("files.read", { workspace: "test", path: "shop/.chauffeur/settings.toml" });
    expect(document.body.textContent).toContain("Create shop/.chauffeur/settings.toml in your editor");
    expect(document.body.textContent).toContain('[scripts.run.dev]');
    expect(call.mock.calls.some(([method]) => ["files.open", "process.run", "chat.send", "chat.create"].includes(method))).toBe(false);
    missing = false;
    await click('[data-testid="no-run-scripts"] button');
    expect(useApp.getState().filesRequest.test).toMatchObject({ kind: "open", path: "shop/.chauffeur/settings.toml", line: null });
    expect(call.mock.calls.some(([method]) => method === "files.open")).toBe(false);
  } finally { session.close(); call.mockRestore(); }
});

domTest("an archive toast keeps its navigation action until dismissed", file, async () => {
  const { toast, dismissToast, useToasts } = await import("@/state/toasts");
  let opened = false;
  const timer = spyOn(globalThis, "setTimeout");
  try {
    toast("Workspace archived", "info", { label: "View archive", run: () => { opened = true; } });
    const notice = useToasts.getState().toasts.at(-1)!;
    expect(timer).not.toHaveBeenCalled();
    expect(notice.action?.label).toBe("View archive");
    notice.action?.run();
    expect(opened).toBe(true);
    dismissToast(notice.id);
    expect(useToasts.getState().toasts.some((item) => item.id === notice.id)).toBe(false);
  } finally { timer.mockRestore(); }
});

domTest("global toast feedback stays outside a modal's inert background and its action is focusable", file, async ({ render, click }) => {
  const { Toasts } = await import("@/App");
  const { Modal } = await import("./ui");
  const { toast, useToasts } = await import("@/state/toasts");
  let opened = false;
  useToasts.setState({ toasts: [] });
  toast("Workspace archived", "info", { label: "View archive", run: () => { opened = true; } });
  await render(<><Toasts /><Modal open title="Settings" onClose={() => {}}><button>Setting</button></Modal></>);
  const feedback = document.querySelector<HTMLElement>('[data-global-feedback]')!;
  expect(feedback.parentElement).toBe(document.body);
  expect(feedback.closest('[inert]')).toBeNull();
  expect(feedback.getAttribute("aria-live")).toBe("polite");
  expect(feedback.querySelector('[role="status"]')?.textContent).toContain("Workspace archived");
  const action = feedback.querySelector('button')!;
  action.focus();
  expect(document.activeElement).toBe(action);
  await click('[data-global-feedback] button');
  expect(opened).toBe(true);
});

domTest("connection recovery stays accessible over a modal and reserves space for its banner", file, async ({ render, click }) => {
  const { ConnectionBanner } = await import("@/App");
  const { Modal } = await import("./ui");
  const { client, useApp } = await import("@/state/app");
  useApp.setState({ connection: { status: "connecting", attempt: 3, error: "test daemon offline" }, workspacesLoaded: true });
  const retry = spyOn(client, "retry").mockImplementation(() => {});
  const bounds = spyOn(window.HTMLElement.prototype, "getBoundingClientRect").mockReturnValue({ x: 200, y: 0, top: 0, left: 200, right: 1000, bottom: 80, width: 800, height: 80, toJSON: () => ({}) });
  const previous = Object.getOwnPropertyDescriptor(globalThis, "ResizeObserver");
  Object.defineProperty(globalThis, "ResizeObserver", { configurable: true, value: window.ResizeObserver });
  try {
    await render(<><ConnectionBanner /><Modal open title="Settings" onClose={() => {}}><button>Setting</button></Modal></>);
    const banner = document.querySelector<HTMLElement>('[data-testid="connection-banner"]')!;
    expect(banner.parentElement).toBe(document.body);
    expect(banner.closest('[inert]')).toBeNull();
    expect(banner.style.left).toBe("200px");
    expect(document.documentElement.style.getPropertyValue("--global-feedback-height")).toBe("80px");
    expect(document.querySelector<HTMLElement>('[data-testid="connection-placeholder"]')?.style.height).toBe("80px");
    const summary = banner.querySelector('summary')!;
    summary.focus();
    expect(document.activeElement).toBe(summary);
    await click('[data-testid="connection-banner"] summary');
    expect(banner.querySelector('details')?.open).toBe(true);
    expect(banner.textContent).toContain("test daemon offline");
    await click('[data-testid="connection-banner"] button');
    expect(retry).toHaveBeenCalledTimes(1);
  } finally {
    retry.mockRestore(); bounds.mockRestore();
    if (previous) Object.defineProperty(globalThis, "ResizeObserver", previous);
    else Reflect.deleteProperty(globalThis, "ResizeObserver");
  }
});
