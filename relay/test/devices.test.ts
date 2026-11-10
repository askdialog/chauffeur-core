import { expect, test } from "bun:test";
import { joinRelay, randomToken } from "@chauffeur/remote";
import { startRelay } from "../src/server";

test("device admission survives QR expiry and reconnects; renewal preserves sessions and deletion revokes", async () => {
  const adminToken = randomToken();
  const relay = startRelay({ adminToken, port: 0, roomLifetimeMs: 30, maxRooms: 1 });
  const origin = `ws://127.0.0.1:${relay.server.port}`;
  const http = `http://127.0.0.1:${relay.server.port}`;
  const config = { deviceId: randomToken(16), hostToken: randomToken(), phoneToken: randomToken() };
  const register = (body = config, authorized = true) => fetch(`${http}/v1/devices`, { method: "POST", headers: { Authorization: authorized ? `Bearer ${adminToken}` : "" }, body: JSON.stringify(body) });
  const pair = () => Promise.all([joinRelay(origin, config.deviceId, "host", config.hostToken, undefined, AbortSignal.timeout(1000), "devices"), joinRelay(origin, config.deviceId, "phone", config.phoneToken, undefined, AbortSignal.timeout(1000), "devices")]);
  try {
    expect((await register(config, false)).status).toBe(401);
    expect((await register()).status).toBe(200);
    expect((await register({ ...config, deviceId: randomToken(16) })).status).toBe(503);
    let [host, phone] = await pair();
    await Bun.sleep(60);
    expect(host.status).toBe("open"); expect(phone.status).toBe("open");
    expect((await register()).status).toBe(200);
    expect(host.status).toBe("open");
    const received = host[Symbol.asyncIterator]().next(); phone.send(Uint8Array.of(1, 2, 3));
    expect(Array.from((await received).value!.subarray())).toEqual([1, 2, 3]);
    await phone.close();
    for (let i = 0; i < 100 && host.status === "open"; i++) await Bun.sleep(5);
    [host, phone] = await pair();
    expect(host.status).toBe("open");
    expect((await fetch(`${http}/v1/devices/${config.deviceId}`, { method: "DELETE" })).status).toBe(401);
    expect((await fetch(`${http}/v1/devices/${config.deviceId}`, { method: "DELETE", headers: { Authorization: `Bearer ${adminToken}` } })).status).toBe(200);
    for (let i = 0; i < 100 && phone.status === "open"; i++) await Bun.sleep(5);
    expect(phone.status).not.toBe("open");
    const retry = await joinRelay(origin, config.deviceId, "phone", config.phoneToken, undefined, AbortSignal.timeout(500), "devices").then(() => null, (error: Error) => error);
    expect(retry).toBeInstanceOf(Error);
  } finally { relay.stop(); }
});

test("a silent mobile socket expires even while its peer keeps sending, freeing the reconnect slot", async () => {
  const adminToken = randomToken();
  const relay = startRelay({ adminToken, port: 0, peerTimeoutMs: 120 });
  const origin = `ws://127.0.0.1:${relay.server.port}`;
  const config = { deviceId: randomToken(16), hostToken: randomToken(), phoneToken: randomToken() };
  const sockets: WebSocket[] = [];
  let keepalive: ReturnType<typeof setInterval> | undefined;
  async function admit(role: "host" | "phone", token: string) {
    const socket = new WebSocket(`${origin}/v1/devices/${config.deviceId}`);
    sockets.push(socket);
    socket.onopen = () => socket.send(JSON.stringify({ role, token }));
    return { socket, ready: new Promise<void>((resolve, reject) => {
      socket.onmessage = event => { if (event.data === '{"type":"ready"}') resolve(); };
      socket.onerror = () => reject(new Error("socket failed"));
    }) };
  }
  try {
    await fetch(`${origin.replace("ws:", "http:")}/v1/devices`, { method: "POST", headers: { Authorization: `Bearer ${adminToken}` }, body: JSON.stringify(config) });
    const host = await admit("host", config.hostToken);
    const phone = await admit("phone", config.phoneToken);
    await Promise.all([host.ready, phone.ready]);
    const gone = new Promise<void>(resolve => phone.socket.onclose = () => resolve());
    keepalive = setInterval(() => {
      if (host.socket.readyState === WebSocket.OPEN) host.socket.send('{"type":"ping"}');
    }, 20);
    // No close/error and no traffic from the phone: models a blackholed Wi-Fi TCP
    // socket. The old relay held this role until the OS timed it out minutes later.
    await Promise.race([gone, Bun.sleep(1500).then(() => { throw new Error("Silent phone still occupies the role"); })]);
    clearInterval(keepalive);
    const nextHost = await admit("host", config.hostToken);
    const nextPhone = await admit("phone", config.phoneToken);
    await Promise.all([nextHost.ready, nextPhone.ready]);
    expect(nextPhone.socket.readyState).toBe(WebSocket.OPEN);
    expect(nextHost.socket.readyState).toBe(WebSocket.OPEN);
  } finally { clearInterval(keepalive); for (const socket of sockets) socket.close(); relay.stop(); }
});
