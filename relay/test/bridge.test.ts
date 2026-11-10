import { afterEach, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createServer } from "node:http";
import { WebSocketServer } from "ws";
import { generateIdentity, peerIdFromPrivateKey, randomToken, joinRelay, securePairing, JsonChannel, HISTORY_PAGE_TOO_LARGE } from "@chauffeur/remote";
import { startRelay } from "../src/server";
import { allowedRequest, createBridge, phoneCanvas } from "../src/bridge";
import { connectDaemon, openPreview } from "../src/daemon";
const cleanup: (() => void | Promise<void>)[] = [];
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); });
async function fixture(adaptiveHistory = false) {
  const folder = await mkdtemp(join(tmpdir(), "chauffeur-mobile-"));
  cleanup.push(() => rm(folder, { recursive: true, force: true }));
  const socket = join(folder, "daemon.sock");
  const server = createServer();
  // The daemon's `/preview` route: refused for a preview not declared, else its bytes echoed.
  const sockets = new WebSocketServer({ server, verifyClient: (info, done) => info.req.url === "/preview/denied/0" ? done(false, 404, "no such preview declared there") : done(true) });
  cleanup.push(() => { for (const client of sockets.clients) client.terminate(); sockets.close(); server.close(); });
  const calls: string[] = [];
  sockets.on("connection", (ws, upgrade) => upgrade.url?.startsWith("/preview/") ? ws.on("message", (data: Buffer) => {
    if (data.length) ws.send(Buffer.concat([Buffer.from(`echo ${upgrade.url}:`), data])); else ws.close();
  }) : ws.on("message", (data) => {
    const request = JSON.parse(data.toString()); calls.push(request.method);
    const results: Record<string, unknown> = { hello: { protocol_version: 1 }, "workspace.list": { workspaces: [{ id: "w", title: "Private workspace" }] }, "events.history": { events: [{ seq: 1, text: "🛠️".repeat(adaptiveHistory && request.params.limit > 25 ? 90_000 : 30_000) }] }, "events.subscribe": { subscription: "s", last_seq: 1 }, "chat.send": { id: "c" } };
    const event = (subscription: string, event: unknown) => ws.send(JSON.stringify({ jsonrpc: "2.0", method: "event", params: { subscription, envelope: { seq: null, event } } }));
    if (request.method === "events.subscribe" && request.params.workspaces.length) {
      event("t", { scope: "workspace", workspace_id: "w", event: { type: "terminal_output", terminal_id: "x", data: "secret" } });
      ws.send(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { subscription: "t", last_seq: 1 } }));
      setTimeout(() => {
        event("t", { scope: "chat", workspace_id: "w", chat_id: "other", event: { type: "text_delta", message_id: "b", text: "Other chat" } });
        event("t", { scope: "workspace", workspace_id: "w", event: { type: "process_output", process_id: "p", line: "secret" } });
        event("t", { scope: "workspace", workspace_id: "w", event: { type: "transfer", transfer: { id: "tr", phase: "running", log: ["Stopping on this Mac"] } } });
      }, 20);
      return;
    }
    if (request.method === "events.subscribe") ws.send(JSON.stringify({ jsonrpc: "2.0", method: "event", params: { subscription: "s", envelope: { seq: null, event: { scope: "chat", chat_id: "c", event: { type: "text_delta", message_id: "a", text: "Live reply" } } } } }));
    ws.send(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: results[request.method] ?? {} }));
  }));
  await new Promise<void>((resolve) => server.listen(socket, resolve));
  const adminToken = randomToken(); const relay = startRelay({ adminToken, port: 0 }); cleanup.push(() => relay.stop());
  const origin = `ws://127.0.0.1:${relay.server.port}`;
  const response = await fetch(`http://127.0.0.1:${relay.server.port}/v1/rooms`, { method: "POST", headers: { Authorization: `Bearer ${adminToken}` } });
  const room = await response.json() as { room: string; hostToken: string; phoneToken: string; expiresAt: number };
  const hostKey = await generateIdentity("Ed25519"); const phoneKey = await generateIdentity("Ed25519");
  const invitation = { version: 1 as const, relay: origin, room: room.room, phoneToken: room.phoneToken, hostPeerId: peerIdFromPrivateKey(hostKey).toString(), pairingKey: randomToken(), expiresAt: room.expiresAt };
  const [h, p] = await Promise.all([joinRelay(origin, room.room, "host", room.hostToken), joinRelay(origin, room.room, "phone", room.phoneToken)]);
  const [host, phone] = await Promise.all([securePairing(h, hostKey, invitation, "host"), securePairing(p, phoneKey, invitation, "phone")]);
  const hostChannel = new JsonChannel(host.connection); const phoneChannel = new JsonChannel(phone.connection);
  cleanup.push(() => { hostChannel.close(); phoneChannel.close(); });
  const bridge = createBridge(hostChannel, () => connectDaemon(socket), undefined, { openPreview: (workspace, offset, handle) => openPreview(socket, workspace, offset, handle) });
  const messages: any[] = []; phoneChannel.onmessage = (message) => messages.push(message);
  async function waitFor(predicate: (message: any) => boolean) {
    for (let i = 0; i < 200; i++) { const found = messages.find(predicate); if (found) return found; await Bun.sleep(5); }
    throw new Error("Expected session message did not arrive");
  }
  let id = 0;
  async function request(method: string, params: unknown) { const current = ++id; phoneChannel.send({ type: "rpc", id: current, method, params }); return waitFor((message) => message.type === "result" && message.id === current); }
  return { calls, bridge, request, waitFor, phoneChannel, messages };
}
test("requires approval then reads history and streams over Noise through the Unix endpoint", async () => {
  const f = await fixture();
  f.phoneChannel.send({ type: "check" });
  expect((await f.waitFor((message) => message.type === "approval_required")).type).toBe("approval_required");
  expect((await f.request("workspace.list", {})).error).toContain("Approve"); expect(f.calls).toEqual([]);
  await f.bridge.approve(); await f.waitFor((m) => m.type === "authorized");
  expect((await f.request("workspace.list", { include_archived: false })).result.workspaces[0].title).toBe("Private workspace");
  expect((await f.request("events.history", { chat: "c", limit: 25 })).result.events[0].text).toBe("🛠️".repeat(30_000));
  await f.request("events.subscribe", { all: false, registry: false, workspaces: [], chats: ["c"], ephemeral: true, after_seq: 1 });
  expect((await f.waitFor((m) => m.type === "event")).event.params.envelope.event.event.text).toBe("Live reply");
  for (const method of ["chat.send", "chat.interrupt", "chat.respond"]) expect((await f.request(method, { chat: "c", text: "Hello", request_id: "r", response: { type: "allow", always: false } })).error).toBeUndefined();
  expect(f.calls).toContain("chat.send");
  let closed = false; f.phoneChannel.onclose = () => { closed = true; }; f.bridge.close();
  for (let i = 0; i < 100 && !closed; i++) await Bun.sleep(5);
  expect(closed).toBe(true);
}, 10_000);
test("refuses privileged methods and broad history or subscriptions after approval", async () => {
  const f = await fixture(); await f.bridge.approve();
  for (const [method, params] of [["daemon.shutdown", {}], ["files.write", {}], ["terminal.open", {}], ["chat.shell", { chat: "c", command: "id" }], ["settings.get", {}], ["mobile.status", {}], ["mobile.configure", { enabled: true, relay: "wss://relay.example", operator_token: "a".repeat(64) }], ["events.history", { limit: 25 }], ["events.history", { chat: "c", limit: 1000 }], ["events.subscribe", { all: true, chats: [], workspaces: [] }], ["chat.send", { chat: "c", text: "x", images: ["secret"] }]] as const) expect((await f.request(method, params)).error).toBe("Method not available from mobile");
  expect(f.calls).toEqual(["hello"]);
});


test("oversized history pages return a retryable error without closing the encrypted session", async () => {
  const f = await fixture(true); await f.bridge.approve();
  expect((await f.request("events.history", { chat: "c", limit: 200 })).error).toBe(HISTORY_PAGE_TOO_LARGE);
  expect((await f.request("events.history", { chat: "c", limit: 25 })).result.events[0].text).toBe("🛠️".repeat(30_000));
  expect((await f.request("workspace.list", { include_archived: false })).result.workspaces).toHaveLength(1);
});

test("a transfer is started or cancelled between this Mac and a declared machine by name, and its workspace sends only transfer events", async () => {
  const f = await fixture(); await f.bridge.approve();
  const here = { kind: "here" }, vm = { kind: "machine", name: "vm" };
  for (const [method, params] of [
    ["cloud.transfer.machines", {}],
    ["cloud.transfer.status", { workspace: "w" }],
    ["cloud.transfer.start", { workspace: "w", source: here, destination: vm, keep_local_copy: false }],
    ["cloud.transfer.start", { workspace: "w", source: vm, destination: here, keep_local_copy: true }],
    ["cloud.transfer.cancel", { workspace: "w", source: here, destination: vm, keep_local_copy: false }],
    ["cloud.transfer.start", { workspace: "w", source: here, destination: vm, interrupt: true }],
    ["cloud.transfer.interrupt", { workspace: "w" }],
  ] as const) expect((await f.request(method, params)).error).toBeUndefined();
  expect(f.calls.filter((method) => method.startsWith("cloud.transfer."))).toEqual(["cloud.transfer.machines", "cloud.transfer.status", "cloud.transfer.start", "cloud.transfer.start", "cloud.transfer.cancel", "cloud.transfer.start", "cloud.transfer.interrupt"]);
  for (const params of [
    { workspace: "w", source: here, destination: { kind: "socket", path: "/tmp/vm.sock" } },
    { workspace: "w", source: { kind: "socket", path: "/tmp/mac.sock" }, destination: vm },
    { workspace: "w", source: here, destination: vm, controller: "/tmp/controller.sock" },
    { workspace: "w", source: vm, destination: { kind: "machine", name: "gcp" } },
    { workspace: "w", source: here, destination: here },
    { workspace: "w", source: here, destination: { ...vm, path: "/tmp/vm.sock" } },
    { workspace: "w", source: here, destination: vm, keep_local_copy: "yes" },
    { workspace: "w", source: here, destination: vm, interrupt: "now" },
  ]) for (const method of ["cloud.transfer.start", "cloud.transfer.cancel"]) expect((await f.request(method, params)).error).toBe("Method not available from mobile");
  for (const params of [{}, { workspace: 1 }, { workspace: "w", source: here }])
    expect((await f.request("cloud.transfer.interrupt", params)).error).toBe("Method not available from mobile");
  for (const params of [
    { all: false, chats: [], workspaces: ["w"], ephemeral: true, after_seq: 0 },
    { all: false, chats: [], workspaces: ["w", "v"], ephemeral: true },
    { all: false, chats: ["c"], workspaces: ["w"], ephemeral: true },
    { all: false, registry: true, chats: [], workspaces: ["w"], ephemeral: true },
    { all: false, office: true, chats: [], workspaces: ["w"], ephemeral: true },
  ]) expect((await f.request("events.subscribe", params)).error).toBe("Method not available from mobile");
  expect((await f.request("events.subscribe", { all: false, registry: false, chats: [], workspaces: ["w"], ephemeral: true })).result.subscription).toBe("t");
  expect((await f.waitFor((m) => m.type === "event")).event.params.envelope.event.event.type).toBe("transfer");
  await Bun.sleep(30);
  expect(JSON.stringify(f.messages.filter((m) => m.type === "event"))).not.toMatch(/secret|Other chat/);
});
test("a preview's connection passes through the host to the daemon's preview route once approved", async () => {
  const f = await fixture();
  f.phoneChannel.send({ type: "tunnel_open", tunnel: 1, workspace: "shop", offset: 2 });
  expect((await f.waitFor((m) => m.type === "tunnel_close" && m.tunnel === 1)).error).toContain("Approve");
  await f.bridge.approve();
  expect((await f.request("previews.list", { workspace: "shop" })).error).toBeUndefined();
  expect((await f.request("previews.list", { workspace: "shop", socket: "/elsewhere.sock" })).error).toBe("Method not available from mobile");
  f.phoneChannel.send({ type: "tunnel_open", tunnel: 2, workspace: "shop", offset: 2 });
  f.phoneChannel.send({ type: "tunnel_data", tunnel: 2, data: Buffer.from("GET /").toString("base64") });
  const answer = await f.waitFor((m) => m.type === "tunnel_data" && m.tunnel === 2);
  expect(Buffer.from(answer.data, "base64").toString()).toBe("echo /preview/shop/2:GET /");
  expect((await f.waitFor((m) => m.type === "tunnel_ack" && m.tunnel === 2)).bytes).toBe(5);
  f.phoneChannel.send({ type: "tunnel_end", tunnel: 2 });
  await f.waitFor((m) => m.type === "tunnel_close" && m.tunnel === 2);
  f.phoneChannel.send({ type: "tunnel_open", tunnel: 3, workspace: "denied", offset: 0 });
  expect((await f.waitFor((m) => m.type === "tunnel_close" && m.tunnel === 3)).error).toContain("no such preview declared there");
  f.phoneChannel.send({ type: "tunnel_open", tunnel: 4, workspace: "../etc", offset: 0 });
  expect((await f.waitFor((m) => m.type === "tunnel_close" && m.tunnel === 4)).error).toBe("Invalid preview");
});


test("mobile can mark only the named workspace seen after approval", async () => {
  const f = await fixture();
  expect((await f.request("workspace.seen", { workspace: "w" })).error).toContain("Approve");
  await f.bridge.approve();
  expect((await f.request("workspace.seen", { workspace: "w" })).error).toBeUndefined();
  expect(f.calls).toContain("workspace.seen");
  for (const params of [{}, { workspace: 1 }, { workspace: "w", unread: true }])
    expect((await f.request("workspace.seen", params)).error).toBe("Method not available from mobile");
});


test("confirmed sends have the same mobile limits as ordinary text sends", () => {
  expect(allowedRequest("chat.send_confirmed", { chat: "c", text: "hello", client_id: "send" })).toBe(true);
  expect(allowedRequest("chat.send_confirmed", { chat: "c", text: "x".repeat(16001), client_id: "send" })).toBe(false);
  expect(allowedRequest("chat.send_confirmed", { chat: "c", text: "hello", images: [{}] })).toBe(false);
});

test("an approved phone can list workspace models and change one without gaining permission-setting access", async () => {
  const f = await fixture();
  expect((await f.request("models.list", { refresh: false, workspace: "w" })).error).toBe("Approve access on your Mac first");
  await f.bridge.approve();
  for (const [method, params] of [
    ["models.list", { refresh: false, workspace: "w" }],
    ["chat.configure", { chat: "c", model: "opus", effort: "high", fast: false }],
    ["chat.configure", { chat: "c", model: "gpt-6.1-sol", agent: "codex", confirm_agent: "claude" }],
  ] as const) expect((await f.request(method, params)).error).toBeUndefined();
  const count = f.calls.length;
  for (const params of [
    { chat: "c", model: "opus", permission_mode: "bypass_permissions" },
    { chat: "c", permission_mode: "plan" },
    { chat: "c", model: "opus", agent: "unknown" },
    { chat: "c", model: "opus", fast: "yes" },
    { chat: "c", model: "opus", effort: {} },
    { chat: "c", model: "" },
  ]) expect((await f.request("chat.configure", params)).error).toBe("Method not available from mobile");
  expect((await f.request("models.list", { refresh: false, socket: "/tmp/other.sock" })).error).toBe("Method not available from mobile");
  expect(f.calls).toHaveLength(count);
});

test("mobile Canvas reads stay scoped to a chat and bounded offsets without filesystem access", () => {
  expect(allowedRequest("design.canvas", { chat: "c" })).toBe(true);
  expect(allowedRequest("design.resource", { chat: "c", file: "a.html", offset: 0, version: null })).toBe(true);
  for (const params of [{ chat: "c", file: "x", offset: -1, version: null }, { chat: "c", file: "x", offset: 17 * 1024 * 1024, version: null }, { chat: "c", file: "x", offset: 0, version: null, root: "/" }]) expect(allowedRequest("design.resource", params)).toBe(false);
  expect(allowedRequest("design.canvas", { chat: "c", root: "/" })).toBe(false);
  expect(allowedRequest("files.read", { workspace: "w", path: "/etc/hosts" })).toBe(false);
  expect(allowedRequest("design.share_start", { chat: "c" })).toBe(false);
});


test("phone Canvas manifests carry availability without the Mac filesystem capability", () => {
  const screen = { file: "a.html", width: 390, height: 844, url: "http://127.0.0.1:9/daemon-secret/w/c/a.html" };
  const result = phoneCanvas({ chat: "c", dir: "/design", pages: [{ name: "Page", sections: [{ title: "Section", screens: [screen] }] }], unsorted: [screen] });
  expect(result.unsorted[0].url).toBe("canvas:available");
  expect(JSON.stringify(result)).not.toContain("daemon-secret");
  expect(JSON.stringify(result)).not.toContain("127.0.0.1");
  expect(screen.url).toContain("daemon-secret");
});
