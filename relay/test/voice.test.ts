import { expect, setSystemTime, test } from "bun:test";
import { joinRelay, randomToken } from "@chauffeur/remote";
import { mistralSessions } from "../src/mistral";
import { startRelay } from "../src/server";

const TOKEN = "rt_synthetic.short-lived-token";
const good = () => Response.json({ client_secret: { value: TOKEN, expires_at: new Date(Date.now() + 900_000).toISOString() } }, { status: 201 });
type Provider = (signal: AbortSignal) => Promise<Response>;

function fixture(provider: Provider = async () => good(), options: { disabled?: boolean; timeout?: number } = {}) {
  let calls = 0;
  const adminToken = randomToken();
  const relay = startRelay({ adminToken, port: 0, voiceTimeoutMs: options.timeout,
    voice: options.disabled ? undefined : mistralSessions("synthetic-dedicated-provider-key", async (url, init) => {
      calls++;
      expect(url).toBe("https://api.mistral.ai/v1/client/sessions");
      expect(init.method).toBe("POST");
      expect(init.redirect).toBe("error");
      expect(init.headers).toEqual({ Authorization: "Bearer synthetic-dedicated-provider-key", "Content-Type": "application/json" });
      expect(JSON.parse(String(init.body))).toEqual({ purpose: "realtime", model: "voxtral-mini-transcribe-realtime-2602" });
      if (!init.signal) throw new Error("Missing provider deadline");
      return provider(init.signal);
    }),
  });
  const http = `http://127.0.0.1:${relay.server.port}`;
  const ws = `ws://127.0.0.1:${relay.server.port}`;
  const config = () => ({ deviceId: randomToken(16), hostToken: randomToken(), phoneToken: randomToken() });
  type Device = ReturnType<typeof config>;
  const register = (device: Device) => fetch(`${http}/v1/devices`, { method: "POST", headers: { Authorization: `Bearer ${adminToken}` }, body: JSON.stringify(device) });
  const connect = (device: Device) => Promise.all([
    joinRelay(ws, device.deviceId, "host", device.hostToken, undefined, AbortSignal.timeout(1000), "devices"),
    joinRelay(ws, device.deviceId, "phone", device.phoneToken, undefined, AbortSignal.timeout(1000), "devices"),
  ]);
  const mint = (device: Device, credential: string | null = device.phoneToken, suffix = "", body?: string) => fetch(`${http}/v1/devices/${device.deviceId}/voice-session${suffix}`, {
    method: "POST", headers: credential === null ? {} : { Authorization: `Bearer ${credential}` }, body,
  });
  const revoke = (device: Device) => fetch(`${http}/v1/devices/${device.deviceId}`, { method: "DELETE", headers: { Authorization: `Bearer ${adminToken}` } });
  return { relay, http, ws, adminToken, config, register, connect, mint, revoke, get calls() { return calls; } };
}

async function until(predicate: () => boolean) {
  for (let i = 0; i < 200 && !predicate(); i++) await Bun.sleep(2);
  expect(predicate()).toBe(true);
}

test("only a registered connected phone receives the plain ephemeral token without cache or provider data", async () => {
  const f = fixture();
  try {
    const device = f.config();
    expect((await f.mint(device)).status).toBe(401);
    await f.register(device);
    expect((await f.mint(device)).status).toBe(409);
    await f.connect(device);
    for (const credential of [null, "wrong", device.hostToken, f.adminToken, `Bearer ${device.phoneToken}`]) {
      expect((await f.mint(device, credential)).status).toBe(401);
    }
    expect((await f.mint(device, device.phoneToken, "?model=evil")).status).toBe(400);
    expect((await f.mint(device, device.phoneToken, "", "{}")).status).toBe(400);
    expect(f.calls).toBe(0);
    const result = await f.mint(device);
    expect(result.status).toBe(200);
    expect(result.headers.get("Cache-Control")).toBe("no-store");
    expect(result.headers.get("Content-Type")).toContain("text/plain");
    expect(await result.text()).toBe(TOKEN);
    await f.revoke(device);
    expect((await f.mint(device)).status).toBe(401);
    expect(f.calls).toBe(1);
  } finally { f.relay.stop(); }
});

test("pairing rooms and expired registrations cannot mint credentials", async () => {
  const f = fixture();
  try {
    const room = await (await fetch(`${f.http}/v1/rooms`, { method: "POST", headers: { Authorization: `Bearer ${f.adminToken}` } })).json() as { room: string; hostToken: string; phoneToken: string };
    await Promise.all([joinRelay(f.ws, room.room, "host", room.hostToken), joinRelay(f.ws, room.room, "phone", room.phoneToken)]);
    expect((await f.mint({ ...room, deviceId: room.room })).status).toBe(401);
    const device = f.config();
    await f.register(device);
    await f.connect(device);
    setSystemTime(new Date(Date.now() + 24 * 60 * 60_000 + 1));
    expect((await f.mint(device)).status).toBe(401);
    expect(f.calls).toBe(0);
  } finally { setSystemTime(); f.relay.stop(); }
});

test("a relay without a provider key keeps its existing connections and returns unavailable", async () => {
  const f = fixture(undefined, { disabled: true });
  try {
    const device = f.config(); await f.register(device); await f.connect(device);
    const result = await f.mint(device);
    expect(result.status).toBe(503);
    expect(result.headers.get("Cache-Control")).toBe("no-store");
    expect(f.calls).toBe(0);
    expect((await fetch(`${f.http}/health`)).status).toBe(200);
  } finally { f.relay.stop(); }
});

test("device mint attempts are limited and registration renewal does not reset their budget", async () => {
  const f = fixture();
  try {
    const device = f.config(); await f.register(device); await f.connect(device);
    for (let i = 0; i < 6; i++) expect((await f.mint(device)).status).toBe(200);
    await f.register(device);
    expect((await f.mint(device)).status).toBe(429);
    expect(f.calls).toBe(6);
  } finally { f.relay.stop(); }
});

test("the global mint budget limits many individually authorized devices", async () => {
  const f = fixture();
  try {
    for (let i = 0; i < 10; i++) {
      const device = f.config(); await f.register(device); await f.connect(device);
      for (let j = 0; j < 6; j++) expect((await f.mint(device)).status).toBe(200);
    }
    const device = f.config(); await f.register(device); await f.connect(device);
    expect((await f.mint(device)).status).toBe(429);
    expect(f.calls).toBe(60);
  } finally { f.relay.stop(); }
});

test("one device and four devices globally bound concurrent provider calls", async () => {
  const pending: Array<() => void> = [];
  const f = fixture(() => new Promise(resolve => pending.push(() => resolve(good()))));
  try {
    const devices = Array.from({ length: 5 }, () => f.config());
    for (const device of devices) { await f.register(device); await f.connect(device); }
    const first = devices[0]!;
    const requests = [f.mint(first)];
    await until(() => f.calls === 1);
    expect((await f.mint(first)).status).toBe(429);
    for (const device of devices.slice(1, 4)) requests.push(f.mint(device));
    await until(() => f.calls === 4);
    expect((await f.mint(devices[4]!)).status).toBe(429);
    for (const resolve of pending) resolve();
    for (const result of await Promise.all(requests)) expect(result.status).toBe(200);
  } finally { f.relay.stop(); }
});

for (const change of ["revocation", "rotation", "disconnect", "reconnect", "renewal"] as const) {
  test(`a provider response rechecks the same authorization and sockets after ${change}`, async () => {
    let release: (() => void) | undefined;
    const f = fixture(() => new Promise(resolve => { release = () => resolve(good()); }));
    try {
      const device = f.config(); await f.register(device); const [, phone] = await f.connect(device);
      const request = f.mint(device);
      await until(() => f.calls === 1);
      if (change === "revocation") await f.revoke(device);
      if (change === "rotation") await f.register({ ...device, phoneToken: randomToken() });
      if (change === "disconnect") { await phone.close(); await Bun.sleep(10); }
      if (change === "reconnect") {
        await phone.close(); await Bun.sleep(10); await f.connect(device);
      }
      if (change === "renewal") await f.register(device);
      release!();
      const result = await request;
      expect(result.status).toBe(change === "renewal" ? 200 : (change === "disconnect" || change === "reconnect") ? 409 : 401);
      if (change !== "renewal") expect(await result.text()).not.toContain(TOKEN);
    } finally { f.relay.stop(); }
  });
}

test("provider failure, redirects and malformed or expired tokens return only sanitized errors", async () => {
  const values = [
    new Response("private-provider-error", { status: 500 }),
    new Response(null, { status: 302, headers: { Location: "https://evil.example" } }),
    Response.json({ client_secret: { value: TOKEN } }),
    new Response("not JSON", { status: 201 }),
    new Response("x".repeat(16_385), { status: 201 }),
    ...["rt_bad token", "rt_bad,token", "rt_bad/token", "rt_" + "x".repeat(2046), "secret-key", "rt_"].map(value => Response.json({ client_secret: { value } }, { status: 201 })),
    ...["invalid", "2020-01-01T00:00:00Z", 123].map(expires_at => Response.json({ client_secret: { value: TOKEN, expires_at } }, { status: 201 })),
    Response.json({ expires_at: "2020-01-01", client_secret: { value: TOKEN } }, { status: 201 }),
  ];
  for (const value of values) {
    const f = fixture(async () => value);
    try {
      const device = f.config(); await f.register(device); await f.connect(device);
      const result = await f.mint(device);
      expect(result.status).toBe(502);
      expect(await result.text()).toBe("Voice provider unavailable");
      expect(result.headers.get("Cache-Control")).toBe("no-store");
    } finally { f.relay.stop(); }
  }
});

test("a hung provider times out, aborts and releases the inflight slot without leaking its error", async () => {
  let signal: AbortSignal | undefined;
  const f = fixture(async current => { signal = current; return new Promise(() => {}); }, { timeout: 30 });
  try {
    const device = f.config(); await f.register(device); await f.connect(device);
    const started = performance.now();
    const result = await f.mint(device);
    expect(result.status).toBe(504);
    expect(performance.now() - started).toBeLessThan(1000);
    expect(signal?.aborted).toBe(true);
    expect((await f.mint(device)).status).toBe(504);
    expect(f.calls).toBe(2);
  } finally { f.relay.stop(); }
});


test("a provider deadline includes a stalled response body and aborts its stream", async () => {
  let aborted = false;
  const f = fixture(async signal => new Response(new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(new TextEncoder().encode('{"client_secret":'));
      signal.addEventListener("abort", () => { aborted = true; controller.error(new Error("synthetic-private-error")); }, { once: true });
    },
  }), { status: 201 }), { timeout: 30 });
  try {
    const device = f.config(); await f.register(device); await f.connect(device);
    const result = await f.mint(device);
    expect(result.status).toBe(504);
    expect(await result.text()).toBe("Voice provider unavailable");
    expect(aborted).toBe(true);
  } finally { f.relay.stop(); }
});

test("credentials shared with the host or operator are never accepted as phone credentials", async () => {
  const f = fixture();
  try {
    for (const credential of [randomToken(), f.adminToken]) {
      const device = { ...f.config(), hostToken: credential, phoneToken: credential };
      await f.register(device); await f.connect(device);
      expect((await f.mint(device)).status).toBe(401);
    }
    expect(f.calls).toBe(0);
  } finally { f.relay.stop(); }
});
