import { JsonChannel, HISTORY_PAGE_EVENTS, HISTORY_PAGE_BYTES, HISTORY_PAGE_TOO_LARGE, Tunnels, type TunnelEnd, type TunnelHandle } from "@chauffeur/remote";
import type { CanvasResult } from "@chauffeur/protocol";
import { MOBILE_HOST_METHODS, PHONE_METHODS } from "@chauffeur/protocol";
import { connectDaemon, daemonSocket, openPreview, type DaemonConnection } from "./daemon";

// Declared with each method (`#[access(.., phone)]` in chauffeur-protocol).
const METHODS = new Set<string>(PHONE_METHODS);
// The phone's own push notifications, which this host answers (`pushCall`).
const HOST_METHODS = new Set<string>(MOBILE_HOST_METHODS);

// Deliberately scoped: no terminals, filesystem, settings, archive, shutdown or
// arbitrary RPC forwarding. The daemon still validates every method's params.
// Beside them, tunnels (`tunnel_*` messages) carry connections to a moved workspace's declared
// previews only: the daemon's `/preview` route refuses any other port.
export function allowedRequest(method: string, params: unknown): boolean {
  if (!METHODS.has(method) || !params || typeof params !== "object" || Array.isArray(params)) return false;
  const p = params as Record<string, unknown>;
  // A phone may choose the model/provider, never change permission modes or other settings.
  if (method === "chat.configure") return typeof p.chat === "string" && typeof p.model === "string" && p.model.length > 0 &&
    Object.keys(p).every(key => ["chat", "model", "agent", "confirm_agent", "effort", "fast"].includes(key)) &&
    [p.agent, p.confirm_agent].every(agent => agent === undefined || agent === "claude" || agent === "codex") &&
    (p.effort === undefined || typeof p.effort === "string") && (p.fast === undefined || typeof p.fast === "boolean");
  if (method === "design.canvas") return Object.keys(p).length === 1 && typeof p.chat === "string";
  if (method === "design.resource") return Object.keys(p).every(key => ["chat", "file", "offset", "version"].includes(key)) && typeof p.chat === "string" &&
    typeof p.file === "string" && p.file.length > 0 && p.file.length <= 2048 &&
    Number.isInteger(p.offset) && Number(p.offset) >= 0 && Number(p.offset) <= 16 * 1024 * 1024 &&
    (p.version === null || typeof p.version === "string");
  if (method === "models.list") return Object.keys(p).every(key => ["refresh", "workspace"].includes(key)) &&
    typeof p.refresh === "boolean" && (p.workspace === undefined || typeof p.workspace === "string");
  if (method === "events.history") return typeof p.chat === "string" && Number.isInteger(p.limit) && Number(p.limit) >= 1 && Number(p.limit) <= HISTORY_PAGE_EVENTS;
  if (method === "events.subscribe") return p.all === false && Array.isArray(p.chats) && Array.isArray(p.workspaces) && (
    (p.chats.length <= 1 && p.workspaces.length === 0) || (transferSubscription(p) && p.after_seq === undefined));
  if (method === "cloud.transfer.start" || method === "cloud.transfer.cancel") return transferBetweenHereAndMachine(p);
  if (method === "cloud.transfer.interrupt") return Object.keys(p).length === 1 && typeof p.workspace === "string";
  if (method === "previews.list" || method === "workspace.seen") return Object.keys(p).length === 1 && typeof p.workspace === "string";
  if (method === "chat.send" || method === "chat.send_confirmed") return typeof p.text === "string" && p.text.trim().length > 0 && p.text.length <= 16_000 && !p.images;
  return true;
}

/** A phone needs screen availability, never the Mac server's filesystem capability URLs. */
export function phoneCanvas(canvas: CanvasResult): CanvasResult {
  const screen = (item: CanvasResult["unsorted"][number]) => ({ ...item, url: item.url ? "canvas:available" : undefined });
  return { ...canvas, pages: canvas.pages.map(page => ({ ...page, sections: page.sections.map(section => ({ ...section, screens: section.screens.map(screen) })) })), unsorted: canvas.unsorted.map(screen) };
}

// One workspace, to follow its transfer: only its `transfer` events reach the phone. No
// registry or Office scope, whose events could pass before the subscription's id is known.
function transferSubscription(p: Record<string, unknown>) {
  return Array.isArray(p.chats) && p.chats.length === 0 && Array.isArray(p.workspaces) && p.workspaces.length === 1 && typeof p.workspaces[0] === "string" && !p.registry && !p.office;
}

// The daemon takes daemon sockets and another controller from its Unix endpoint, which this host
// uses: they are the CLI's. The phone, as the desktop app, names this Mac and a declared machine.
function transferBetweenHereAndMachine(p: Record<string, unknown>) {
  if (Object.keys(p).some((key) => !["workspace", "source", "destination", "keep_local_copy", "interrupt"].includes(key))) return false;
  if (typeof p.workspace !== "string") return false;
  if ([p.keep_local_copy, p.interrupt].some((flag) => flag !== undefined && typeof flag !== "boolean")) return false;
  const ends = [endpoint(p.source), endpoint(p.destination)];
  return ends.includes("here") && ends.includes("machine");
}
function endpoint(value: unknown): "here" | "machine" | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const end = value as Record<string, unknown>;
  const keys = Object.keys(end).length;
  if (end.kind === "here" && keys === 1) return "here";
  if (end.kind === "machine" && typeof end.name === "string" && keys === 2) return "machine";
  return null;
}

// A workspace's other events carry terminal and script output. A transfer subscription's chat
// events are dropped once its id is known; any before that are chats the phone may read anyway.
function forwarded(event: unknown, transferSubscriptions: Set<string>) {
  const params = (event as { params?: { subscription?: string; envelope?: { event?: { scope?: string; event?: { type?: string } } } } } | null)?.params;
  const scoped = params?.envelope?.event;
  if (scoped?.scope === "workspace") return scoped.event?.type === "transfer";
  return !transferSubscriptions.has(params?.subscription ?? "");
}

export function createBridge(channel: JsonChannel, connect: () => Promise<DaemonConnection> = connectDaemon, onClose: (reason: string) => void = () => {}, options: {
  onGreeting?: () => Promise<void>;
  authorization?: Record<string, unknown>;
  beforeCall?: () => Promise<void>;
  pushCall?: (method: string, params: unknown) => Promise<unknown>;
  openPreview?: (workspace: string, offset: number, handle: TunnelHandle) => Promise<TunnelEnd>;
} = {}) {
  let daemon: DaemonConnection | undefined;
  let closed = false;
  let approving = false;
  const pending = new Set<number>();
  const subscriptions = new Set<string>();
  const transferSubscriptions = new Set<string>();
  let greeted = false;
  const tunnels = new Tunnels((message) => channel.send(message), async (workspace, offset, handle) => {
    await options.beforeCall?.();
    if (closed || !daemon) throw new Error("Session ended");
    return (options.openPreview ?? ((w, o, h) => openPreview(daemonSocket(), w, o, h)))(workspace, offset, handle);
  });
  const close = (reason = "disconnected") => { if (!closed) { closed = true; tunnels.closeAll(); onClose(reason); daemon?.close(); channel.close(); } };
  channel.onclose = () => close(channel.failure ?? "transport_closed");
  channel.onmessage = (value) => {
    if (closed) return;
    const message = value as { type?: string; id?: number; method?: string; params?: unknown } | null;
    if (message?.type === "check") {
      if (!greeted) {
        greeted = true;
        if (options.onGreeting) void options.onGreeting().catch(() => close("authorization_failed"));
        else channel.send({ type: "approval_required" });
      }
      return;
    }
    if (typeof message?.type === "string" && message.type.startsWith("tunnel_")) {
      if (message.type === "tunnel_open" && !daemon) {
        try { channel.send({ type: "tunnel_close", tunnel: (message as { tunnel?: unknown }).tunnel, error: "Approve access on your Mac first" }); } catch { close(); }
        return;
      }
      tunnels.receive(message);
      return;
    }
    if (!message || message.type !== "rpc" || !Number.isSafeInteger(message.id) || message.id! < 1) { close("invalid_request"); return; }
    const id = message.id!;
    const reply = (result: unknown, error?: string) => {
      if (closed) return;
      try { channel.send(error ? { type: "result", id, error } : { type: "result", id, result }); }
      catch { close(); }
    };
    if (!daemon) { reply(null, "Approve access on your Mac first"); return; }
    if (!message.method || !(allowedRequest(message.method, message.params) || (options.pushCall && HOST_METHODS.has(message.method)))) { reply(null, "Method not available from mobile"); return; }
    if (pending.has(id) || pending.size >= 8 || (message.method === "events.subscribe" && subscriptions.size + pending.size >= 8)) { close(); return; }
    pending.add(id);
    const connection = daemon;
    void (async () => {
      await options.beforeCall?.();
      if (closed) throw new Error("Session ended");
      if (options.pushCall && HOST_METHODS.has(message.method!)) return options.pushCall(message.method!, message.params);
      return connection.call(message.method!, message.params);
    })().then((result) => {
      if (message.method === "events.subscribe") {
        const { subscription } = result as { subscription: string };
        subscriptions.add(subscription);
        if (transferSubscription(message.params as Record<string, unknown>)) transferSubscriptions.add(subscription);
      }
      if (message.method === "events.unsubscribe") {
        const { subscription } = message.params as { subscription: string };
        subscriptions.delete(subscription); transferSubscriptions.delete(subscription);
      }
      if (message.method === "events.history" && Buffer.byteLength(JSON.stringify({ type: "result", id, result }), "utf8") > HISTORY_PAGE_BYTES) {
        reply(null, HISTORY_PAGE_TOO_LARGE);
      } else reply(message.method === "design.canvas" ? phoneCanvas(result as CanvasResult) : result);
    }, (error: Error) => reply(null, error.message)).finally(() => pending.delete(id));
  };
  return {
    close,
    async approve(authorization: Record<string, unknown> = options.authorization ?? {}) {
      if (closed || daemon || approving) throw new Error("No session to approve");
      approving = true;
      try {
        const connection = await connect();
        if (closed) { connection.close(); throw new Error("Session ended before approval"); }
        daemon = connection;
        daemon.onClose = () => close("daemon_closed");
        daemon.onEvent = (event) => {
          if (!forwarded(event, transferSubscriptions)) return;
          try { channel.send({ type: "event", event }); } catch { close(); }
        };
        channel.send({ type: "authorized", ...authorization });
      } catch (error) { close(); throw error; }
      finally { approving = false; }
    },
  };
}
