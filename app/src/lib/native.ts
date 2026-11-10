// Native concerns, through Tauri when the UI runs in the app and through browser fallbacks when it
// runs in a plain browser during development. Everything else goes to the daemon over WebSocket.

import type { ShortcutId } from "@chauffeur/protocol";
import { browserConnection, inBrowserOffice } from "./browserOffice";
import type { FileDrop } from "./fileDrops";
import type { SnapshotRequest } from "./screenSnapshots";
import type { ConnectionInfo } from "./rpc";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
    /** Injected by the Vite dev server (see vite.config.ts). */
    __CHAUFFEUR_DEV_CONNECTION__?: ConnectionInfo;
  }
}

export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Where the daemon listens. The Tauri shell starts the daemon when it is not running. */
export async function resolveConnection(): Promise<ConnectionInfo> {
  if (isTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<ConnectionInfo>("daemon_connection");
  }
  if (inBrowserOffice) return browserConnection();
  // The dev server reads $CHAUFFEUR_HOME afresh on every call (the daemon may have restarted on
  // another port); the connection injected into the page is only a fallback for when the dev
  // server itself is gone.
  let response: Response;
  try {
    response = await fetch("/__chauffeur/connection", { cache: "no-store" });
  } catch (error) {
    if (window.__CHAUFFEUR_DEV_CONNECTION__) return window.__CHAUFFEUR_DEV_CONNECTION__;
    throw error;
  }
  const body = (await response.json().catch(() => ({}))) as ConnectionInfo & { error?: string };
  if (!response.ok) throw new Error(body.error ?? `HTTP ${response.status}`);
  return body;
}

/** Zooms the whole page: the web view's own zoom in the app, CSS zoom in a plain browser. */
export async function setAppZoom(scale: number): Promise<void> {
  if (isTauri) {
    const { getCurrentWebview } = await import("@tauri-apps/api/webview");
    await getCurrentWebview().setZoom(scale);
  } else {
    document.documentElement.style.zoom = scale === 1 ? "" : String(scale);
  }
}

/** Only the Linux app has a tray icon: macOS says what needs the user with the Dock's badge. */
const hasTray = isTauri && /Linux/.test(navigator.platform || navigator.userAgent);

/** Tells the tray icon how many workspaces need the user. The Linux app only; never throws. */
export async function setTrayAttention(count: number): Promise<void> {
  if (!hasTray) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("set_attention", { count }).catch((error: unknown) => console.warn("tray update failed", error));
}

export function windowFocused(): boolean {
  return typeof document !== "undefined" && document.hasFocus() && document.visibilityState === "visible";
}

let permission: Promise<boolean> | null = null;

async function notificationsAllowed(): Promise<boolean> {
  if (isTauri) {
    const api = await import("@tauri-apps/plugin-notification");
    if (await api.isPermissionGranted()) return true;
    return (await api.requestPermission()) === "granted";
  }
  if (typeof Notification === "undefined") return false;
  if (Notification.permission === "granted") return true;
  if (Notification.permission === "denied") return false;
  return (await Notification.requestPermission()) === "granted";
}

/** Shows a macOS notification (a web notification in a browser). Never throws. */
export async function notify(title: string, body: string): Promise<void> {
  try {
    permission ??= notificationsAllowed();
    if (!(await permission)) return;
    if (isTauri) {
      const api = await import("@tauri-apps/plugin-notification");
      api.sendNotification({ title, body });
    } else {
      new Notification(title, { body });
    }
  } catch (error) {
    console.warn("notification failed", error);
  }
}

/** The macOS app's menu: the keys of its items follow the user's (see src-tauri/src/menu.rs). */
const hasMenu = isTauri && /Mac/.test(navigator.platform || navigator.userAgent);
const menuKeys: { bindings: Partial<Record<ShortcutId, string>>; recording: boolean; terminal: boolean; member: boolean } = {
  bindings: {},
  recording: false,
  terminal: false,
  member: false,
};

/**
 * Gives the macOS menu's items the keys in force: the user's `bindings`, or none while Settings ›
 * Keyboard is `recording` keys (the menu would take them first); New Terminal and Close Terminal
 * instead of New Chat and Close Chat while the Terminal view is the one in use (`terminal`); the
 * Office and Connect Phone… items for a `member` only. Never throws.
 */
export function setMenuKeys(change: Partial<typeof menuKeys>): void {
  Object.assign(menuKeys, change);
  if (!hasMenu) return;
  const { bindings, recording, terminal, member } = menuKeys;
  void import("@tauri-apps/api/core")
    .then(({ invoke }) => invoke("set_menu_keys", { bindings, recording, terminal, member }))
    .catch((error: unknown) => console.warn("menu keys update failed", error));
}

/** Menu actions of the app menu (Tauri only); returns an unlisten function. */
export function onMenuAction(handler: (action: string) => void): () => void {
  if (!isTauri) return () => {};
  let unlisten: (() => void) | null = null;
  let cancelled = false;
  void import("@tauri-apps/api/event").then(({ listen }) =>
    listen<string>("menu", (event) => handler(event.payload)).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    }),
  );
  return () => {
    cancelled = true;
    unlisten?.();
  };
}

/** Whether files dropped from the desktop come through the shell (Linux's WebKitGTK; see fileDrops.ts). */
export const nativeFileDrops = isTauri && /Linux/.test(navigator.platform || navigator.userAgent);

/** Files dropped on the window from the desktop (Linux only); returns an unlisten function. */
export function onFileDrop(handler: (drop: FileDrop) => void): () => void {
  if (!nativeFileDrops) return () => {};
  let unlisten: (() => void) | null = null;
  let cancelled = false;
  void import("@tauri-apps/api/event").then(({ listen }) =>
    listen<FileDrop>("file-drop", (event) => handler(event.payload)).then((fn) => {
      if (cancelled) fn();
      else unlisten = fn;
    }),
  );
  return () => {
    cancelled = true;
    unlisten?.();
  };
}

/** The `index`th file of a native drop, read by the shell. */
export async function readDroppedFile(drop: FileDrop, index: number): Promise<File> {
  const file = drop.files[index];
  const { invoke } = await import("@tauri-apps/api/core");
  const bytes = await invoke<ArrayBuffer>("read_dropped_file", { drop: drop.drop, index });
  return new File([bytes], file.name, { type: file.mediaType });
}

/** Whether the app pictures a design chat's screens natively for the canvas (macOS's WKWebView). */
export const picturesScreens = isTauri && typeof navigator !== "undefined" && /Macintosh/.test(navigator.userAgent);

/** A PNG of `rect` of a canvas screen (its frame's URL, laid out at `width` × `height`), `pixels` wide. */
export async function screenSnapshot(request: Pick<SnapshotRequest, "src" | "width" | "height" | "rect" | "pixels">): Promise<Blob> {
  const { invoke } = await import("@tauri-apps/api/core");
  const bytes = await invoke<ArrayBuffer>("screen_snapshot", {
    url: request.src,
    width: request.width,
    height: request.height,
    rect: request.rect,
    pixels: request.pixels,
  });
  return new Blob([bytes], { type: "image/png" });
}

/** Closes the views that picture screens, once no canvas shows. */
export async function releaseScreenSnapshots(): Promise<void> {
  if (!picturesScreens) return;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("screen_snapshots_release");
}

/** Opens a URL in the default browser. */
export async function openExternal(url: string): Promise<void> {
  if (isTauri) {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("open_url", { url });
  } else {
    window.open(url, "_blank", "noopener");
  }
}

/**
 * Quits the app and opens it again, to run the version an update put in its place. Returns false
 * in a plain browser, which cannot.
 */
export async function relaunchApp(): Promise<boolean> {
  if (!isTauri) return false;
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("relaunch_app");
  return true;
}

/** Phone pairing stays in an unprivileged Chauffeur window, never a browser. */
export async function openMobileConnection(): Promise<void> {
  if (!isTauri) throw new Error("Open the Chauffeur app to connect your phone.");
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("open_mobile_connection");
}

/** Listen before draining: links received during desktop startup remain available. */
export async function onWorkspaceShareLink(onLink: (url: string) => void): Promise<() => void> {
  if (!isTauri) return () => {};
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  const take = async () => {
    const link = await invoke<string | null>("take_workspace_share_link");
    if (link) onLink(link);
  };
  const unlisten = await listen("workspace-share-link", () => { void take().catch(console.error); });
  await take();
  return unlisten;
}
