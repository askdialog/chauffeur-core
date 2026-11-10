// The phone's side of a preview (docs/workspace-relay.md, decided with Justin 2026-10-27): while
// its screen is open, the phone listens on its own loopback, on the same port numbers as the Mac's
// (so that a dev server's redirects, cookies and login callbacks naming localhost:<port> work as
// they are), and carries each connection the web view opens through a tunnel of the encrypted
// session. A port the phone already uses falls back to a free one. Nothing listens beyond
// loopback, and nothing once the screen closes. No platform code: the server comes from node's
// `net` in tests, react-native-tcp-socket on the phone.
import type { Tunnels } from "./tunnel";

/** What the proxy needs of an accepted socket; node's `net.Socket` and react-native-tcp-socket's
 * both have it. */
export interface LocalSocket {
  on(event: "data", listener: (data: Uint8Array | string) => void): unknown;
  on(event: "end" | "close", listener: () => void): unknown;
  on(event: "error", listener: (error: Error) => void): unknown;
  write(data: Uint8Array, encoding: undefined, callback: (error?: Error | null) => void): unknown;
  end(): unknown;
  destroy(): unknown;
  pause(): unknown;
  resume(): unknown;
}

export interface LocalServer {
  listen(options: { port: number; host: string }, callback?: () => void): unknown;
  once(event: "error", listener: (error: Error) => void): unknown;
  address(): { port: number } | string | null;
  close(): unknown;
}

/** A loopback server whose connections stay half-open when the client ends its writes. */
export type CreateServer = (onConnection: (socket: LocalSocket) => void) => LocalServer;

export interface PreviewProxy {
  /** The phone's port of each preview, by its place in the workspace's block. */
  ports: Map<number, number>;
  close(): void;
}

function listen(server: LocalServer, port: number) {
  return new Promise<number>((resolve, reject) => {
    server.once("error", reject);
    server.listen({ port, host: "127.0.0.1" }, () => {
      const address = server.address();
      resolve(typeof address === "object" && address ? address.port : port);
    });
  });
}

/** Listens for each of `workspace`'s previews, passing every connection through `tunnels` to the
 * preview at the same place of its block. `tunnels` is asked at each connection: the session's
 * after a reconnect; one that cannot open (no session) refuses the connection. */
export async function servePreviews(
  createServer: CreateServer,
  tunnels: () => Pick<Tunnels, "open">,
  workspace: string,
  previews: { offset: number; port: number }[],
): Promise<PreviewProxy> {
  const servers: LocalServer[] = [];
  const sockets = new Set<LocalSocket>();
  const ports = new Map<number, number>();
  const close = () => {
    for (const server of servers) server.close();
    for (const socket of sockets) socket.destroy();
    servers.length = 0; sockets.clear();
  };
  try {
    for (const { offset, port } of previews) {
      const serve = () => createServer((socket) => {
        let opened: Pick<Tunnels, "open">;
        try { opened = tunnels(); }
        catch { socket.destroy(); return; }
        sockets.add(socket);
        const tunnel = opened.open(workspace, offset, {
          write: (bytes, written) => {
            // A socket closed meanwhile throws; its close follows.
            try { socket.write(bytes, undefined, (error) => { if (!error) written(); }); } catch {}
          },
          end: () => { socket.end(); },
          destroy: () => { socket.destroy(); },
          pause: () => { socket.pause(); },
          resume: () => { socket.resume(); },
        });
        socket.on("data", (data) => tunnel.data(typeof data === "string" ? new TextEncoder().encode(data) : data));
        socket.on("end", () => tunnel.ended());
        socket.on("close", () => { sockets.delete(socket); tunnel.closed(); });
        // A close follows.
        socket.on("error", () => {});
      });
      let server = serve();
      let bound: number;
      try { bound = await listen(server, port); }
      catch {
        // Something of the phone holds that port: any free one.
        server.close();
        server = serve();
        bound = await listen(server, 0);
      }
      servers.push(server);
      ports.set(offset, bound);
    }
  } catch (error) { close(); throw error; }
  return { ports, close };
}
