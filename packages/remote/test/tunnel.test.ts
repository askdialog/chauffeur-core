import { afterEach, expect, test } from "bun:test";
import { connect, createServer, type Server, type Socket } from "node:net";
import { servePreviews, type CreateServer, type PreviewProxy } from "../src/preview-proxy";
import { TUNNEL_CHUNK, TUNNEL_WINDOW, Tunnels, type TunnelEnd, type TunnelHandle } from "../src/tunnel";

const cleanup: (() => void)[] = [];
afterEach(() => { for (const done of cleanup.splice(0)) done(); });

const loopback: CreateServer = (onConnection) => createServer({ allowHalfOpen: true }, onConnection);

// What the Mac host does with an accepted tunnel, the daemon replaced by a TCP connection.
function tcpEnd(port: number, handle: TunnelHandle): Promise<TunnelEnd> {
  return new Promise((resolve, reject) => {
    const socket: Socket = connect({ port, host: "127.0.0.1", allowHalfOpen: true });
    socket.once("error", reject);
    socket.once("connect", () => {
      socket.on("data", (data: Buffer) => handle.data(data));
      socket.on("end", () => handle.ended());
      socket.on("close", () => handle.closed());
      resolve({
        write: (bytes, written) => { socket.write(bytes, () => written()); },
        end: () => { socket.end(); },
        destroy: () => { socket.destroy(); },
        pause: () => { socket.pause(); },
        resume: () => { socket.resume(); },
      });
    });
  });
}

// A phone and a Mac host joined by an in-order message channel; `inFlight` follows the bytes the
// host sent the phone and the phone has not acknowledged.
function session(accept: (workspace: string, offset: number, handle: TunnelHandle) => Promise<TunnelEnd>) {
  const seen = { opened: [] as [string, number][], inFlight: 0, maxInFlight: 0 };
  let phone!: Tunnels, host!: Tunnels;
  const deliver = (to: () => Tunnels) => (message: unknown) => queueMicrotask(() => to().receive(structuredClone(message)));
  phone = new Tunnels((message) => {
    const m = message as { type: string; bytes?: number };
    if (m.type === "tunnel_ack") seen.inFlight -= m.bytes!;
    deliver(() => host)(message);
  });
  host = new Tunnels((message) => {
    const m = message as { type: string; data?: string };
    if (m.type === "tunnel_data") {
      seen.inFlight += Buffer.from(m.data!, "base64").length;
      seen.maxInFlight = Math.max(seen.maxInFlight, seen.inFlight);
    }
    deliver(() => phone)(message);
  }, (workspace, offset, handle) => { seen.opened.push([workspace, offset]); return accept(workspace, offset, handle); });
  return { phone, host, seen };
}

function server(handler: (socket: Socket) => void): Promise<{ port: number; server: Server }> {
  return new Promise((resolve) => {
    const listening = createServer({ allowHalfOpen: true }, handler);
    listening.listen(0, "127.0.0.1", () => resolve({ port: (listening.address() as { port: number }).port, server: listening }));
    cleanup.push(() => listening.close());
  });
}

async function proxy(tunnels: Tunnels, port: number): Promise<PreviewProxy> {
  const served = await servePreviews(loopback, () => tunnels, "shop", [{ offset: 2, port }]);
  cleanup.push(() => served.close());
  return served;
}

function exchange(port: number, request: string, shutdown = false): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    const socket = connect({ port, host: "127.0.0.1", allowHalfOpen: true });
    const chunks: Buffer[] = [];
    socket.on("data", (chunk: Buffer) => chunks.push(chunk));
    // As a browser does once the answer ended.
    socket.on("end", () => socket.end());
    // Reset by the phone: what came is the answer.
    socket.on("error", () => resolve(Buffer.concat(chunks)));
    socket.on("close", () => resolve(Buffer.concat(chunks)));
    socket.write(request, (error) => { if (error) reject(error); else if (shutdown) socket.end(); });
  });
}

test("a connection to the phone's loopback reaches the preview at the same place of the block", async () => {
  const dev = await server((socket) => socket.once("data", () => socket.end("HTTP/1.0 200 OK\r\n\r\nserved there")));
  const { phone, seen } = session((_, __, handle) => tcpEnd(dev.port, handle));
  const free = await server(() => {});
  free.server.close();
  const served = await proxy(phone, free.port);
  // Same port as the Mac's when the phone has it free.
  expect(served.ports.get(2)).toBe(free.port);
  const answer = await exchange(free.port, "GET / HTTP/1.0\r\n\r\n");
  expect(answer.toString()).toContain("served there");
  expect(seen.opened).toEqual([["shop", 2]]);
});

test("a client that ends its writes still reads the answer", async () => {
  const dev = await server((socket) => {
    const chunks: Buffer[] = [];
    socket.on("data", (chunk: Buffer) => chunks.push(chunk));
    socket.on("end", () => socket.end(`got ${Buffer.concat(chunks).toString()}`));
  });
  const { phone } = session((_, __, handle) => tcpEnd(dev.port, handle));
  const served = await proxy(phone, 0);
  const answer = await exchange(served.ports.get(2)!, "ping", true);
  expect(answer.toString()).toBe("got ping");
});

test("a large answer flows with at most a window of unacknowledged bytes", async () => {
  const body = Buffer.alloc(3 * 1024 * 1024 + 123, 7);
  const dev = await server((socket) => socket.once("data", () => socket.end(body)));
  const { phone, seen } = session((_, __, handle) => tcpEnd(dev.port, handle));
  const served = await proxy(phone, 0);
  const answer = await exchange(served.ports.get(2)!, "GET");
  expect(answer.equals(body)).toBe(true);
  expect(seen.maxInFlight).toBeLessThanOrEqual(TUNNEL_WINDOW + TUNNEL_CHUNK);
  expect(seen.inFlight).toBe(0);
});

test("a preview the Mac refuses closes the phone's connection, and a port in use falls back", async () => {
  const { phone, host } = session(async () => { throw new Error("no such preview declared there"); });
  const taken = await server(() => {});
  const served = await proxy(phone, taken.port);
  expect(served.ports.get(2)).not.toBe(taken.port);
  expect((await exchange(served.ports.get(2)!, "GET / HTTP/1.0\r\n\r\n")).length).toBe(0);
  await Bun.sleep(10);
  expect(phone.size + host.size).toBe(0);
});

test("ending the session closes every open connection", async () => {
  const dev = await server(() => {});
  const { phone, host } = session((_, __, handle) => tcpEnd(dev.port, handle));
  const served = await proxy(phone, 0);
  const pending = exchange(served.ports.get(2)!, "GET / HTTP/1.0\r\n\r\n");
  for (let i = 0; i < 100 && host.size === 0; i++) await Bun.sleep(5);
  expect(host.size).toBe(1);
  phone.closeAll(); host.closeAll();
  expect((await pending).length).toBe(0);
});

test("the Mac refuses a tunnel to an invalid preview or past the limit", () => {
  const sent: unknown[] = [];
  const host = new Tunnels((message) => sent.push(message), () => new Promise(() => {}));
  host.receive({ type: "tunnel_open", tunnel: 1, workspace: "../etc", offset: 0 });
  host.receive({ type: "tunnel_open", tunnel: 2, workspace: "shop", offset: 1024 });
  for (let id = 3; id < 3 + 12; id++) host.receive({ type: "tunnel_open", tunnel: id, workspace: "shop", offset: 0 });
  host.receive({ type: "tunnel_open", tunnel: 99, workspace: "shop", offset: 0 });
  expect(sent).toEqual([
    { type: "tunnel_close", tunnel: 1, error: "Invalid preview" },
    { type: "tunnel_close", tunnel: 2, error: "Invalid preview" },
    { type: "tunnel_close", tunnel: 99, error: "Too many preview connections at once" },
  ]);
  expect(host.receive({ type: "rpc", id: 1 })).toBe(false);
  // The phone accepts no tunnel.
  const phoneSent: unknown[] = [];
  new Tunnels((message) => phoneSent.push(message)).receive({ type: "tunnel_open", tunnel: 1, workspace: "shop", offset: 0 });
  expect(phoneSent).toEqual([{ type: "tunnel_close", tunnel: 1, error: "This side opens no tunnel" }]);
});
