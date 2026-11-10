import { afterEach, describe, expect, test } from "bun:test";
import {
  generateIdentity, joinRelay, parseInvitation, peerIdFromPrivateKey,
  randomToken, securePairing, type Invitation, type Socket,
} from "@chauffeur/remote";
import { startRelay, type RelayLog } from "../src/server";

const relays: ReturnType<typeof startRelay>[] = [];
afterEach(() => { for (const relay of relays.splice(0)) relay.stop(); });

async function setup(telemetry?: (event: RelayLog) => void) {
  const adminToken = randomToken();
  const relay = startRelay({ adminToken, port: 0, telemetry });
  relays.push(relay);
  const relayOrigin = `ws://127.0.0.1:${relay.server.port}`;
  const response = await fetch(`http://127.0.0.1:${relay.server.port}/v1/rooms`, {
    method: "POST", headers: { Authorization: `Bearer ${adminToken}` },
  });
  expect(response.status).toBe(201);
  const room = await response.json() as { room: string; hostToken: string; phoneToken: string; expiresAt: number };
  const hostIdentity = await generateIdentity("Ed25519");
  const phoneIdentity = await generateIdentity("Ed25519");
  const invitation: Invitation = {
    version: 1, relay: relayOrigin, room: room.room, phoneToken: room.phoneToken,
    hostPeerId: peerIdFromPrivateKey(hostIdentity).toString(), pairingKey: randomToken(), expiresAt: room.expiresAt,
  };
  return { relay, relayOrigin, adminToken, room, hostIdentity, phoneIdentity, invitation };
}

async function rawPair(fixture: Awaited<ReturnType<typeof setup>>, hostSocket?: (url: string) => Socket) {
  return Promise.all([
    joinRelay(fixture.relayOrigin, fixture.room.room, "host", fixture.room.hostToken, hostSocket),
    joinRelay(fixture.relayOrigin, fixture.room.room, "phone", fixture.room.phoneToken),
  ]);
}

function captureSocket(capture: Uint8Array[], transform?: (data: Uint8Array) => Uint8Array[]) {
  return (url: string): Socket => {
    const socket = new WebSocket(url);
    const originalSend = socket.send.bind(socket);
    socket.send = ((data: string | Uint8Array) => {
      if (typeof data === "string") { originalSend(data); return; }
      const bytes = data.slice();
      capture.push(bytes);
      for (const packet of transform?.(bytes) ?? [bytes]) originalSend(new Uint8Array(packet).buffer);
    }) as typeof socket.send;
    return socket as unknown as Socket;
  };
}

describe("opaque relay and Noise pairing", () => {
  test("requires room-creation authentication and bounds capacity", async () => {
    const adminToken = randomToken();
    const relay = startRelay({ adminToken, port: 0, maxRooms: 1 });
    relays.push(relay);
    const url = `http://127.0.0.1:${relay.server.port}/v1/rooms`;
    expect((await fetch(url, { method: "POST" })).status).toBe(401);
    const init = { method: "POST", headers: { Authorization: `Bearer ${adminToken}` } };
    expect((await fetch(url, init)).status).toBe(201);
    expect((await fetch(url, init)).status).toBe(503);
  });

  test("exchanges encrypted data and exposes no plaintext to the relay transport", async () => {
    const logs: RelayLog[] = [];
    const f = await setup(event => logs.push(event));
    const captured: Uint8Array[] = [];
    const [hostRaw, phoneRaw] = await rawPair(f, captureSocket(captured));
    const [host, phone] = await Promise.all([
      securePairing(hostRaw, f.hostIdentity, f.invitation, "host"),
      securePairing(phoneRaw, f.phoneIdentity, f.invitation, "phone"),
    ]);
    expect(phone.remotePeer.toString()).toBe(f.invitation.hostPeerId);
    expect(host.remotePeer.toString()).toBe(peerIdFromPrivateKey(f.phoneIdentity).toString());
    const message = "private-agent-conversation-" + randomToken();
    const incoming = phone.connection[Symbol.asyncIterator]().next();
    host.connection.send(new TextEncoder().encode(message));
    const received = await incoming;
    expect(new TextDecoder().decode(received.value!.subarray())).toBe(message);
    const visible = captured.map((bytes) => new TextDecoder().decode(bytes)).join("");
    expect(visible).not.toContain(message);
    expect(visible).not.toContain(f.invitation.pairingKey);
    await host.connection.close();
    for (let i = 0; i < 40 && !logs.some(event => event.event === "connection_closed"); i++) await Bun.sleep(5);
    expect(logs.some(event => event.event === "role_authenticated" && event.role === "host")).toBe(true);
    const closed = logs.find(event => event.event === "connection_closed" && event.role === "host");
    expect(closed?.forwardedBytes).toBeGreaterThan(0);
    expect(closed?.durationMs).toBeGreaterThanOrEqual(0);
    const logged = JSON.stringify(logs);
    for (const secret of [message, f.adminToken, f.room.room, f.room.hostToken, f.room.phoneToken, f.invitation.pairingKey, f.invitation.hostPeerId]) {
      expect(logged).not.toContain(secret);
    }
  });

  test("rejects inconsistent host invitations even when a relay admits an impostor", async () => {
    const f = await setup();
    const attacker = await generateIdentity("Ed25519");
    const [hostRaw, phoneRaw] = await rawPair(f);
    const attackerInvite = { ...f.invitation, hostPeerId: peerIdFromPrivateKey(attacker).toString() };
    const results = await Promise.allSettled([
      securePairing(hostRaw, attacker, attackerInvite, "host", AbortSignal.timeout(1000)),
      securePairing(phoneRaw, f.phoneIdentity, f.invitation, "phone", AbortSignal.timeout(1000)),
    ]);
    expect(results.every((result) => result.status === "rejected")).toBe(true);
  });

  test("relay role tokens alone do not authorize a phone without the QR secret", async () => {
    const f = await setup();
    const [hostRaw, phoneRaw] = await rawPair(f);
    const results = await Promise.allSettled([
      securePairing(hostRaw, f.hostIdentity, f.invitation, "host", AbortSignal.timeout(1000)),
      securePairing(phoneRaw, f.phoneIdentity, { ...f.invitation, pairingKey: randomToken() }, "phone", AbortSignal.timeout(1000)),
    ]);
    expect(results.every((result) => result.status === "rejected")).toBe(true);
  });

  for (const mode of ["tamper", "replay"] as const) {
    test(`rejects ${mode} of encrypted application data`, async () => {
      const f = await setup();
      let armed = false;
      const [hostRaw, phoneRaw] = await rawPair(f, captureSocket([], (bytes) => {
        if (!armed) return [bytes];
        if (mode === "replay") return [bytes, bytes];
        const changed = bytes.slice();
        changed[changed.length - 1] ^= 1;
        return [changed];
      }));
      const [host, phone] = await Promise.all([
        securePairing(hostRaw, f.hostIdentity, f.invitation, "host"),
        securePairing(phoneRaw, f.phoneIdentity, f.invitation, "phone"),
      ]);
      const received: Uint8Array[] = [];
      phone.connection.addEventListener("message", (event) => received.push(event.data.subarray()));
      const failed = new Promise<void>((resolve, reject) => {
        const timeout = setTimeout(() => reject(new Error("Tampered session was not closed")), 1000);
        phone.connection.addEventListener("close", (event) => {
          clearTimeout(timeout);
          if (!event.error) reject(new Error("Expected an authenticated-decryption error"));
          else resolve();
        }, { once: true });
      });
      armed = true;
      host.connection.send(new TextEncoder().encode("only-once"));
      await failed;
      expect(received.length).toBe(mode === "replay" ? 1 : 0);
    });
  }

  test("invalid room credentials fail without terminating an existing session", async () => {
    const f = await setup();
    const hostPending = joinRelay(f.relayOrigin, f.room.room, "host", f.room.hostToken);
    await expect(joinRelay(f.relayOrigin, f.room.room, "phone", randomToken())).rejects.toThrow();
    const [hostRaw, phoneRaw] = await Promise.all([
      hostPending, joinRelay(f.relayOrigin, f.room.room, "phone", f.room.phoneToken),
    ]);
    const [host] = await Promise.all([
      securePairing(hostRaw, f.hostIdentity, f.invitation, "host"),
      securePairing(phoneRaw, f.phoneIdentity, f.invitation, "phone"),
    ]);
    await host.connection.close();
  });

  test("rejects expired invitations, unsafe relay origins and altered protocol versions", async () => {
    const f = await setup();
    expect(() => parseInvitation(JSON.stringify(f.invitation))).not.toThrow();
    for (const changed of [
      { expiresAt: Date.now() - 1 }, { relay: "ws://example.com" },
      { relay: "wss://relay.example/?token=secret" }, { version: 2 },
      { hostPeerId: "untrusted" }, { pairingKey: "short" },
    ]) expect(() => parseInvitation(JSON.stringify({ ...f.invitation, ...changed }))).toThrow();
  });

  test("expires abandoned rooms and makes room identifiers unusable", async () => {
    const adminToken = randomToken();
    const relay = startRelay({ adminToken, port: 0, maxRooms: 1, roomLifetimeMs: 20 });
    relays.push(relay);
    const url = `http://127.0.0.1:${relay.server.port}/v1/rooms`;
    const init = { method: "POST", headers: { Authorization: `Bearer ${adminToken}` } };
    const first = await (await fetch(url, init)).json() as { room: string };
    await new Promise((resolve) => setTimeout(resolve, 40));
    expect((await fetch(`${url}/${first.room}`)).status).toBe(404);
    expect((await fetch(url, init)).status).toBe(201);
  });

  test("a completed room cannot be reused after disconnect", async () => {
    const f = await setup();
    const [hostRaw, phoneRaw] = await rawPair(f);
    const phoneClosed = new Promise<void>((resolve) => phoneRaw.addEventListener("close", () => resolve(), { once: true }));
    await hostRaw.close();
    await phoneClosed;
    expect((await fetch(`http://127.0.0.1:${f.relay.server.port}/v1/rooms/${f.room.room}`)).status).toBe(404);
  });
});
