// The UI of the installed extensions, loaded at run time from their verified packages.
//
// The daemon serves each installed revision under its own URL (`ExtensionUi.base_url`). Loading
// one adds its stylesheet, then runs its `extension.js`, which calls `registerExtension` (sdk.ts)
// with what it contributes. Loading is transactional: the new revision replaces the previous one
// only once both its stylesheet and its script loaded and it registered; on any failure the
// previous revision stays, whole, and the failure is kept to be shown.
//
// A view renders an extension's components only while it holds a view lease (`extensions.watch`):
// the daemon then keeps that revision installed, so an update never swaps the code under an open
// view (a SQL draft, an Office call). Leases are counted per extension and given back when the
// last view using them unmounts.

import { useSyncExternalStore, type ComponentType } from "react";
import type { EventEnvelope, ExtensionId, ExtensionUi, JsonValue, WorkspaceInfo } from "@chauffeur/protocol";
import { errorMessage, type Lease, type LeaseState } from "@/lib/rpc";

/** What an extension's UI may use of Core besides the shared modules. */
export type ExtensionHost = {
  readonly id: ExtensionId;
  /** The installed package revision the UI was loaded from. */
  readonly revision: string;
  /** The URL of a file of the package's assets folder, such as `office/garden.glb`. */
  asset(path: string): string;
  /** Calls the extension's engine (`extensions.call`). */
  call(method: string, params: JsonValue, workspace?: string | null): Promise<JsonValue>;
  /** The extension's own events, while one of its views is open. Returns the unsubscribe. */
  onEvent(handler: (event: JsonValue) => void): () => void;
};

/** What an extension's UI adds to Core. */
export type ExtensionContribution = {
  /** A view of each workspace (Database). */
  workspace?: ComponentType<{ workspace: WorkspaceInfo }>;
  /** A view over the whole app (the Office). */
  global?: ComponentType;
  /** The page the browser shows from the extension's restricted listener (the Office). */
  browser?: ComponentType;
  /** Names the quick switcher offers while the global view is shown. */
  members?: readonly string[];
};

export type ExtensionSetup = (host: ExtensionHost) => ExtensionContribution;

/** What the registry needs of the daemon connection, given once by the app. */
export type RegistryClient = {
  call(method: string, params: JsonValue, workspace: string | null, id: ExtensionId): Promise<JsonValue>;
  watch(id: ExtensionId, onEvent: (envelope: EventEnvelope) => void, onState: (state: LeaseState) => void): Lease;
};

/** A `<script>` or `<link>` the page holds for a loaded revision. */
type Element = { remove(): void };

export type LoadedExtension = {
  revision: string;
  contribution: ExtensionContribution;
  script: Element;
  stylesheet: Element | null;
};

/** How a revision's files reach the page: tags in the document, or a test's fake. */
export type UiLoader = {
  stylesheet(id: ExtensionId, href: string): Promise<Element>;
  script(id: ExtensionId, src: string, host: ExtensionHost): Promise<{ element: Element; contribution: ExtensionContribution }>;
};

export type ExtensionUiState = {
  loaded: LoadedExtension | null;
  /** The last revision that could not be loaded, and why. */
  failed: { revision: string; error: string } | null;
};

type Slot = ExtensionUiState & { queue: Promise<unknown> };

const EMPTY: ExtensionUiState = { loaded: null, failed: null };
const slots = new Map<ExtensionId, Slot>();
const listeners = new Set<() => void>();
const handlers = new Map<ExtensionId, Set<(event: JsonValue) => void>>();
let client: RegistryClient | null = null;
let loader: UiLoader = { stylesheet: addStylesheet, script: runScript };

/** Scripts being run, by the token on their `<script>` tag: `registerExtension` finds its own. */
const running = new Map<string, { id: ExtensionId; host: ExtensionHost; contribution: ExtensionContribution | null; error: string | null }>();

export function setRegistryClient(next: RegistryClient) {
  client = next;
}

export function setUiLoader(next: UiLoader) {
  loader = next;
}

function slotOf(id: ExtensionId): Slot {
  let slot = slots.get(id);
  if (!slot) {
    slot = { loaded: null, failed: null, queue: Promise.resolve() };
    slots.set(id, slot);
  }
  return slot;
}

function changed() {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function extensionUi(id: ExtensionId): ExtensionUiState {
  const slot = slots.get(id);
  return slot ? { loaded: slot.loaded, failed: slot.failed } : EMPTY;
}

const snapshots = new Map<ExtensionId, ExtensionUiState>();

/** The loaded UI of an extension; re-renders when another revision replaces it. */
export function useExtensionUi(id: ExtensionId): ExtensionUiState {
  return useSyncExternalStore(subscribe, () => {
    const slot = slots.get(id);
    if (!slot) return EMPTY;
    const previous = snapshots.get(id);
    if (previous && previous.loaded === slot.loaded && previous.failed === slot.failed) return previous;
    const next = { loaded: slot.loaded, failed: slot.failed };
    snapshots.set(id, next);
    return next;
  });
}

/** Where a path `base_url` (the browser listener's) resolves: the page. */
const pageUrl = () => globalThis.location?.href ?? "http://127.0.0.1/";

/** A URL of the package, which must stay beneath its base: metadata cannot point elsewhere. */
export function packageUrl(ui: Pick<ExtensionUi, "base_url">, path: string, origin = pageUrl()): string {
  const base = new URL(ui.base_url.endsWith("/") ? ui.base_url : `${ui.base_url}/`, origin);
  const url = new URL(path, base);
  if (url.origin !== base.origin || !url.pathname.startsWith(base.pathname) || /(^|\/)\.\.?(\/|$)/.test(path)) {
    throw new Error(`The extension refers to ${path}, outside its package`);
  }
  return url.href;
}

export function assetUrl(ui: Pick<ExtensionUi, "base_url" | "assets">, path: string, origin = pageUrl()): string {
  return packageUrl(ui, `${ui.assets.replace(/\/+$/, "")}/${path.replace(/^\/+/, "")}`, origin);
}

/**
 * Makes `ui` the extension's loaded revision, unless it already is. Loads one revision at a time
 * per extension; resolves once it is the loaded one, rejects (previous revision kept) on failure.
 */
export function loadExtensionUi(id: ExtensionId, ui: ExtensionUi): Promise<LoadedExtension> {
  const slot = slotOf(id);
  const attempt = slot.queue.then(() => (slot.loaded?.revision === ui.revision ? slot.loaded : load(id, ui, slot)));
  slot.queue = attempt.catch(() => {});
  return attempt;
}

async function load(id: ExtensionId, ui: ExtensionUi, slot: Slot): Promise<LoadedExtension> {
  let stylesheet: Element | null = null;
  let script: Element | null = null;
  try {
    const host = makeHost(id, ui);
    if (ui.stylesheet) stylesheet = await loader.stylesheet(id, packageUrl(ui, ui.stylesheet));
    const { element, contribution } = await loader.script(id, packageUrl(ui, ui.script), host);
    script = element;
    const previous = slot.loaded;
    const loaded: LoadedExtension = { revision: ui.revision, contribution, script, stylesheet };
    slot.loaded = loaded;
    slot.failed = null;
    previous?.stylesheet?.remove();
    previous?.script.remove();
    changed();
    return loaded;
  } catch (error) {
    stylesheet?.remove();
    script?.remove();
    slot.failed = { revision: ui.revision, error: errorMessage(error) };
    changed();
    throw error;
  }
}

function makeHost(id: ExtensionId, ui: ExtensionUi): ExtensionHost {
  return {
    id,
    revision: ui.revision,
    asset: (path) => assetUrl(ui, path),
    call: (method, params, workspace = null) => {
      if (!client) return Promise.reject(new Error("Chauffeur is not connected"));
      return client.call(method, params, workspace, id);
    },
    onEvent: (handler) => {
      const set = handlers.get(id) ?? new Set();
      handlers.set(id, set);
      set.add(handler);
      return () => set.delete(handler);
    },
  };
}

function addStylesheet(id: ExtensionId, href: string): Promise<Element> {
  return new Promise((resolve, reject) => {
    const link = document.createElement("link");
    link.rel = "stylesheet";
    link.referrerPolicy = "no-referrer";
    link.href = href;
    link.dataset.chauffeurExtension = id;
    link.onload = () => resolve(link);
    link.onerror = () => {
      link.remove();
      reject(new Error(`Could not load the stylesheet of the ${id} extension`));
    };
    document.head.appendChild(link);
  });
}

/**
 * The SDK bridge, imported with the first extension that loads: it holds every shared module
 * (all of CodeMirror's languages among them), which an app showing no extension never needs.
 */
let bridge: Promise<void> | null = null;

function ensureBridge(): Promise<void> {
  bridge ??= import("./bridge").then(({ installBridge }) => installBridge());
  // A failed import (the app was updated under the page) is tried again by the next load.
  bridge.catch(() => {
    bridge = null;
  });
  return bridge;
}

async function runScript(id: ExtensionId, src: string, host: ExtensionHost): Promise<{ element: Element; contribution: ExtensionContribution }> {
  await ensureBridge();
  return new Promise((resolve, reject) => {
    const token = crypto.randomUUID();
    const entry = { id, host, contribution: null as ExtensionContribution | null, error: null as string | null };
    running.set(token, entry);
    const script = document.createElement("script");
    // The script's own exception reaches the window, not the element.
    const onError = (event: ErrorEvent) => {
      if (event.filename === src) entry.error ??= event.message;
    };
    const done = () => {
      running.delete(token);
      window.removeEventListener("error", onError);
    };
    window.addEventListener("error", onError);
    // CORS mode, so that an exception the script throws reports its file rather than "Script error."
    script.crossOrigin = "anonymous";
    script.referrerPolicy = "no-referrer";
    script.src = src;
    script.async = true;
    script.dataset.chauffeurLoad = token;
    script.onload = () => {
      done();
      if (entry.contribution && !entry.error) resolve({ element: script, contribution: entry.contribution });
      else {
        script.remove();
        reject(new Error(`The ${id} extension did not start: ${entry.error ?? "its interface did not register"}`));
      }
    };
    script.onerror = () => {
      done();
      script.remove();
      reject(new Error(`Could not load the interface of the ${id} extension`));
    };
    document.head.appendChild(script);
  });
}

/** The host of the extension whose script is running now, else null (sdk.ts). */
export function currentHost(): ExtensionHost | null {
  const token = (document.currentScript as HTMLScriptElement | null)?.dataset.chauffeurLoad;
  return (token && running.get(token)?.host) || null;
}

/** Called by an extension's script while it runs (sdk.ts). */
export function acceptRegistration(id: ExtensionId, setup: ExtensionSetup) {
  const token = (document.currentScript as HTMLScriptElement | null)?.dataset.chauffeurLoad;
  const entry = token ? running.get(token) : undefined;
  if (!entry) throw new Error("registerExtension must be called while Chauffeur loads the extension");
  if (entry.id !== id) {
    entry.error = `the package of ${entry.id} registered as ${id}`;
    return;
  }
  try {
    entry.contribution = setup(entry.host);
  } catch (error) {
    entry.error = errorMessage(error);
  }
}

// ---- View leases ----------------------------------------------------------------------------

export type LeaseView = { state: LeaseState; generation: number };

type LeaseSlot = LeaseView & { lease: Lease; count: number };

const leases = new Map<ExtensionId, LeaseSlot>();
const leaseListeners = new Set<() => void>();
const NOT_HELD: LeaseView = { state: { held: false, error: null }, generation: 0 };

function leaseChanged() {
  for (const listener of leaseListeners) listener();
}

function deliver(id: ExtensionId, envelope: EventEnvelope) {
  if (envelope.event.scope !== "extension" || envelope.event.id !== id) return;
  for (const handler of handlers.get(id) ?? []) {
    try {
      handler(envelope.event.event);
    } catch (error) {
      console.error(`an event handler of the ${id} extension failed`, error);
    }
  }
}

/** Takes (or shares) the extension's view lease until the returned release is called. */
export function holdLease(id: ExtensionId): () => void {
  let slot = leases.get(id);
  if (!slot) {
    if (!client) throw new Error("Chauffeur is not connected");
    const created: LeaseSlot = { state: { held: false, error: null }, generation: 0, count: 0, lease: null as unknown as Lease };
    leases.set(id, created);
    created.lease = client.watch(
      id,
      (envelope) => deliver(id, envelope),
      (state) => {
        if (leases.get(id) !== created) return;
        created.state = state;
        if (state.held) created.generation += 1;
        leaseChanged();
      },
    );
    slot = created;
  }
  slot.count += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    const current = leases.get(id);
    if (!current) return;
    current.count -= 1;
    if (current.count > 0) return;
    leases.delete(id);
    current.lease.close();
    leaseChanged();
  };
}

/** Asks again now for the leases the daemon refused (the package became ready). */
export function retryLeases() {
  for (const slot of leases.values()) if (!slot.state.held) slot.lease.retry();
}

const leaseView = new Map<ExtensionId, LeaseView>();

/** The extension's lease as views see it; `generation` grows each time it is taken again. */
export function useLease(id: ExtensionId): LeaseView {
  return useSyncExternalStore(
    (listener) => {
      leaseListeners.add(listener);
      return () => leaseListeners.delete(listener);
    },
    () => {
      const slot = leases.get(id);
      if (!slot) return NOT_HELD;
      let view = leaseView.get(id);
      if (!view || view.state !== slot.state || view.generation !== slot.generation) {
        view = { state: slot.state, generation: slot.generation };
        leaseView.set(id, view);
      }
      return view;
    },
  );
}

/** Test support: forgets every loaded revision, lease and handler. */
export function resetRegistry() {
  for (const slot of slots.values()) {
    slot.loaded?.script.remove();
    slot.loaded?.stylesheet?.remove();
  }
  slots.clear();
  snapshots.clear();
  for (const slot of leases.values()) slot.lease.close();
  leases.clear();
  leaseView.clear();
  handlers.clear();
  running.clear();
  loader = { stylesheet: addStylesheet, script: runScript };
  changed();
  leaseChanged();
}
