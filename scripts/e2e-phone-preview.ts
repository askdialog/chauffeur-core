// The phone's side of a moved workspace's preview, for scripts/e2e-cloud-transfer.sh: the phone's
// loopback proxy and tunnels (packages/remote) joined to the Mac's real mobile host bridge
// (relay/src/bridge.ts) on the source daemon's private socket, which reaches the destination's
// dev server. Only the relay and Noise are left out: they carry the bridge's messages opaquely.
//
//   bun scripts/e2e-phone-preview.ts serve <port>
//       a dev server with a login (cookie and redirects) and a live-update WebSocket
//   bun scripts/e2e-phone-preview.ts check <source socket> <workspace>
//       through the phone: the login flow, the WebSocket, an undeclared port refused
//   bun scripts/e2e-phone-preview.ts hold <source socket> <workspace>
//       opens the WebSocket through the phone, says "open", then "closed" once the Mac cuts it
import { createServer } from "node:net";
import { createBridge } from "../relay/src/bridge";
import { connectDaemon, openPreview } from "../relay/src/daemon";
import { servePreviews, Tunnels } from "../packages/remote/src/index";
import type { JsonChannel } from "../packages/remote/src/index";

const [mode, ...args] = process.argv.slice(2);

if (mode === "serve") {
  Bun.serve({
    hostname: "127.0.0.1",
    port: Number(args[0]),
    fetch(request, server) {
      const url = new URL(request.url);
      const user = /(?:^|; )session=([a-z]+)/.exec(request.headers.get("cookie") ?? "")?.[1];
      if (url.pathname === "/live" && server.upgrade(request)) return undefined;
      if (url.pathname === "/login" && request.method === "POST")
        return new Response(null, { status: 303, headers: { Location: "/", "Set-Cookie": "session=alice; Path=/; HttpOnly" } });
      if (url.pathname === "/") return user ? new Response(`hello ${user}`) : Response.redirect("/login", 302);
      return new Response("not found", { status: 404 });
    },
    websocket: { message: (socket, message) => socket.send(`live:${message}`) },
  });
} else {
  const [socket, workspace] = args;
  // The two ends of the encrypted session, in memory: what one sends, the other receives, in order.
  type End = { onmessage: (message: unknown) => void; onclose: () => void; failure: string | null; send: (message: unknown) => void; close: () => void };
  const end = (): End => ({ onmessage: () => {}, onclose: () => {}, failure: null, send: () => {}, close: () => {} });
  const phone = end(), host = end();
  phone.send = (message) => queueMicrotask(() => host.onmessage(structuredClone(message)));
  host.send = (message) => queueMicrotask(() => phone.onmessage(structuredClone(message)));
  // Duck-typed: the bridge uses only these members of a channel.
  const bridge = createBridge(host as unknown as JsonChannel, () => connectDaemon(socket), undefined, {
    openPreview: (w, o, handle) => openPreview(socket, w, o, handle),
  });
  const tunnels = new Tunnels((message) => phone.send(message));
  const answers = new Map<number, (message: { result?: unknown; error?: string }) => void>();
  phone.onmessage = (message) => {
    if (tunnels.receive(message)) return;
    const m = message as { type?: string; id?: number; result?: unknown; error?: string };
    if (m.type === "result") answers.get(m.id!)?.(m);
  };
  await bridge.approve();
  const listed = await new Promise<{ result?: unknown; error?: string }>((resolve) => {
    answers.set(1, resolve);
    phone.send({ type: "rpc", id: 1, method: "previews.list", params: { workspace } });
  });
  if (listed.error) throw new Error(listed.error);
  const { machine, previews } = listed.result as { machine: string | null; previews: { name: string; offset: number; port: number; state: string }[] };
  const web = previews.find((p) => p.name === "web");
  if (machine !== "vm" || !web || web.state !== "ready") throw new Error(`unexpected previews: ${JSON.stringify(listed.result)}`);
  const proxy = await servePreviews((onConnection) => createServer({ allowHalfOpen: true }, onConnection), () => tunnels, workspace, [{ offset: web.offset, port: web.port }]);
  // The Mac's own relay of the port holds its number here: the phone takes another, as on a
  // phone where that port is in use.
  const port = proxy.ports.get(web.offset)!;
  const origin = `http://localhost:${port}`;
  if (mode === "check") {
    const anonymous = await fetch(`${origin}/`, { redirect: "manual" });
    if (anonymous.status !== 302 || anonymous.headers.get("location") !== "/login") throw new Error(`no redirect to the login: ${anonymous.status}`);
    const login = await fetch(`${origin}/login`, { method: "POST", redirect: "manual" });
    const cookie = login.headers.get("set-cookie")?.split(";")[0];
    if (login.status !== 303 || cookie !== "session=alice") throw new Error(`no session cookie: ${login.status} ${cookie}`);
    const home = await fetch(`${origin}${login.headers.get("location")}`, { headers: { cookie } });
    if ((await home.text()) !== "hello alice") throw new Error("the session cookie did not reach the dev server");
    const live = new WebSocket(`ws://localhost:${port}/live`);
    const echoed = await new Promise<string>((resolve, reject) => {
      live.onopen = () => live.send("ping");
      live.onmessage = (event) => resolve(String(event.data));
      live.onerror = () => reject(new Error("the live-update WebSocket failed"));
    });
    if (echoed !== "live:ping") throw new Error(`the live-update WebSocket answered ${echoed}`);
    live.close();
    // A port of the block not declared as a preview there is refused by the Mac.
    const refused = await new Promise<string>((resolve) => {
      tunnels.open(workspace, web.offset + 1, { write: () => {}, end: () => {}, pause: () => {}, resume: () => {}, destroy: () => resolve("closed") });
    });
    if (refused !== "closed") throw new Error("an undeclared port was reached");
    console.log(`ok: ${web.name} on port ${port} of the phone`);
    proxy.close();
    bridge.close();
    process.exit(0);
  }
  const held = new WebSocket(`ws://localhost:${port}/live`);
  held.onopen = () => held.send("hold");
  held.onmessage = () => console.log("open");
  held.onclose = () => { console.log("closed"); process.exit(0); };
}
