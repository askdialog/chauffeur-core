import type Ws from "ws";
import { createConnection } from "node:net";
// Bun's built-in ws shim ignores createConnection. The pinned npm alias preserves exact Unix paths.
const WebSocket: typeof Ws = require("ws-unix");
import { join } from "node:path";
import { homedir } from "node:os";
import type { TunnelEnd, TunnelHandle } from "@chauffeur/remote";
import { PROTOCOL_VERSION } from "@chauffeur/protocol";

export interface DaemonConnection {
  call(method: string, params: unknown): Promise<unknown>;
  onEvent: (message: unknown) => void;
  onClose: () => void;
  close(): void;
}

export const daemonSocket = () => join(process.env.CHAUFFEUR_HOME ?? join(homedir(), ".chauffeur"), "chauffeurd.sock");

// Uses the same private Unix endpoint as the CLI; never reads the daemon token,
// store or conversations from disk, and never auto-starts another daemon.
export async function connectDaemon(socketPath = daemonSocket(), signal?: AbortSignal): Promise<DaemonConnection> {
  const client = await openRoute(socketPath, signal);
  try {
    await client.call("hello", { protocol_version: PROTOCOL_VERSION, client: "chauffeur-mobile-host" });
    signal?.throwIfAborted();
    return client;
  } catch (error) { client.close(); throw error; }
}

async function openRoute(socketPath: string, signal?: AbortSignal): Promise<DaemonConnection> {
  signal?.throwIfAborted();
  const socket = new WebSocket("ws://localhost/ws", { createConnection: () => createConnection({ path: socketPath }), maxPayload: 2 * 1024 * 1024, handshakeTimeout: 5000 });
  const abort = () => socket.terminate();
  signal?.addEventListener("abort", abort, { once: true });
  let nextId = 0;
  const pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  const client: DaemonConnection = {
    onEvent: () => {}, onClose: () => {},
    call(method, params) {
      if (socket.readyState !== WebSocket.OPEN || pending.size >= 16) return Promise.reject(new Error("Daemon unavailable or busy"));
      return new Promise((resolve, reject) => {
        const id = ++nextId;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error("Daemon request timed out")); }, 30_000);
        pending.set(id, { resolve, reject, timer });
        socket.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
      });
    },
    close() { socket.terminate(); },
  };
  socket.on("message", (data) => {
    try {
      const message = JSON.parse(data.toString());
      if (message.method === "event") { client.onEvent(message); return; }
      const entry = pending.get(message.id);
      if (!entry) return;
      pending.delete(message.id); clearTimeout(entry.timer);
      if (message.error) entry.reject(new Error(String(message.error.message)));
      else entry.resolve(message.result);
    } catch { socket.terminate(); }
  });
  socket.on("error", () => {});
  socket.on("close", () => {
    signal?.removeEventListener("abort", abort);
    for (const entry of pending.values()) { clearTimeout(entry.timer); entry.reject(new Error("Daemon disconnected")); }
    pending.clear(); client.onClose();
  });
  try {
    await new Promise<void>((resolve, reject) => {
      socket.once("open", resolve);
      socket.once("error", () => reject(new Error("Cannot connect to Chauffeur on this Mac")));
      socket.once("close", () => reject(new Error("Daemon disconnected during connection")));
    });
    return client;
  } catch (error) { socket.terminate(); throw error; }
}

/** One connection to a moved workspace's declared preview, through the daemon's
 * `/preview/{workspace}/{offset}` on its Unix endpoint: binary messages of its bytes, an empty one
 * for the end of writes. Refused with the daemon's reason (a workspace that runs here, a port not
 * declared there, nothing listening). */
export function openPreview(socketPath: string, workspace: string, offset: number, handle: TunnelHandle): Promise<TunnelEnd> {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(`ws://localhost/preview/${encodeURIComponent(workspace)}/${offset}`, {
      createConnection: () => createConnection({ path: socketPath }), maxPayload: 1024 * 1024, handshakeTimeout: 10_000,
    });
    socket.once("unexpected-response", (request, response) => {
      const chunks: Buffer[] = [];
      response.on("data", (chunk: Buffer) => chunks.push(chunk));
      // Read before the request goes: its body is the daemon's reason.
      response.on("end", () => { request.destroy(); reject(new Error(Buffer.concat(chunks).toString().slice(0, 300) || `The Mac refused the preview (${response.statusCode})`)); });
      response.on("error", () => { request.destroy(); reject(new Error("The Mac refused the preview")); });
    });
    socket.once("error", () => reject(new Error("Cannot reach Chauffeur on this Mac")));
    socket.once("open", () => {
      socket.on("message", (data: Buffer, binary) => {
        if (!binary) return;
        if (data.length) handle.data(data); else handle.ended();
      });
      socket.on("close", () => handle.closed());
      resolve({
        write: (bytes, written) => socket.send(bytes, (error) => { if (!error) written(); }),
        end: () => socket.send(Buffer.alloc(0)),
        destroy: () => socket.terminate(),
        pause: () => socket.pause(),
        resume: () => socket.resume(),
      });
    });
  });
}
