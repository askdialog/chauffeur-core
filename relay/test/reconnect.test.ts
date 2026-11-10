import { afterEach, expect, test } from "bun:test";
import type { RemoteDevice, RemoteStateResult } from "@chauffeur/protocol";
import { generateIdentity, identityFromSeed, peerIdFromPrivateKey, randomToken, joinRelay, securePairing, JsonChannel, SessionManager, type ConnectionState, type RemoteClient, type ResumeConfig } from "@chauffeur/remote";
import { startRelay } from "../src/server";
import { TrustedHost } from "../src/trusted-host";
import { createBridge } from "../src/bridge";
import type { DaemonConnection } from "../src/daemon";

const cleanup: (() => void)[] = [];
afterEach(() => { for (const stop of cleanup.splice(0).reverse()) stop(); });
async function until(predicate: () => boolean, description: string) {
  for (let i = 0; i < 600; i++) { if (predicate()) return; await Bun.sleep(10); }
  throw new Error(`Timed out: ${description}`);
}

async function fixture(saveHook?: (value: ResumeConfig | null) => Promise<void>) {
  const adminToken = randomToken();
  let relay = startRelay({ adminToken, port: 0 }); cleanup.push(() => relay.stop());
  const origin = `ws://127.0.0.1:${relay.server.port}`;
  const state: RemoteStateResult = { identity_seed: Array.from(crypto.getRandomValues(new Uint8Array(32))), devices: [] };
  const methods: string[] = [];
  const connect = async (): Promise<DaemonConnection> => {
    const connection: DaemonConnection = {
      onEvent: () => {}, onClose: () => {}, close: () => {},
      async call(method, raw) {
        methods.push(method);
        const params = raw as any;
        if (method === "remote.state") return structuredClone(state);
        if (method === "remote.approve") {
          const device: RemoteDevice = { info: { id: randomToken(16), peer_id: params.peer_id, name: params.name, relay: params.relay, created_at: new Date().toISOString() }, host_token: randomToken(), phone_token: randomToken(), secret: randomToken() };
          state.devices.push(device); return structuredClone(device);
        }
        if (method === "remote.revoke") { state.devices = state.devices.filter((device) => device.info.id !== params.device_id); return {}; }
        if (method === "remote.check") {
          const device = state.devices.find((device) => device.info.id === params.device_id);
          if (!device) throw new Error("Device is no longer authorized"); return device.info;
        }
        if (method === "workspace.list") return { workspaces: [{ id: "w", title: "Mobile test" }] };
        return {};
      },
    };
    return connection;
  };
  const hostOptions = { relay: origin, adminToken, connect };
  let host = new TrustedHost(hostOptions); host.start(); cleanup.push(() => host.stop());
  const hostKey = await host.hostIdentity();
  const phoneKey = await generateIdentity("Ed25519");
  let saved: ResumeConfig | null = null;
  const states: ConnectionState[] = [];
  let active: RemoteClient | undefined;
  const makeManager = () => {
    const manager = new SessionManager({ identity: async () => phoneKey, load: async () => saved,
      save: async (value) => { await saveHook?.(value); saved = value; },
      onState: (value) => states.push(value), onClient: (value) => { active = value; }, retryDelay: () => 25 });
    cleanup.push(() => manager.shutdown()); return manager;
  };
  let manager = makeManager(); await manager.start();
  async function pair() {
    const response = await fetch(`http://127.0.0.1:${relay.server.port}/v1/rooms`, { method: "POST", headers: { Authorization: `Bearer ${adminToken}` } });
    const room = await response.json() as { room: string; phoneToken: string; hostToken: string; expiresAt: number };
    const invitation = { version: 1 as const, relay: origin, room: room.room, phoneToken: room.phoneToken, hostPeerId: peerIdFromPrivateKey(hostKey).toString(), pairingKey: randomToken(), expiresAt: room.expiresAt };
    const hostConnecting = joinRelay(origin, room.room, "host", room.hostToken);
    manager.pair(JSON.stringify(invitation));
    const hostRaw = await hostConnecting;
    const secured = await securePairing(hostRaw, hostKey, invitation, "host");
    const channel = new JsonChannel(secured.connection); cleanup.push(() => channel.close());
    const bridge = createBridge(channel, connect);
    await until(() => states.at(-1)?.status === "approval", "phone asks for Mac approval");
    expect(saved).toBeNull();
    const profile = await host.approve(secured.remotePeer.toString(), "Pixel");
    await bridge.approve({ resume: profile });
    return profile;
  }
  return {
    pair, states, methods, state, get saved() { return saved; }, get active() { return active; }, get manager() { return manager; },
    restartRelay: () => { const port = relay.server.port!; relay.stop(); relay = startRelay({ adminToken, port }); },
    restartHost: () => { host.stop(); host = new TrustedHost(hostOptions); host.start(); },
    restartPhone: async () => { manager.shutdown(); manager = makeManager(); await manager.start(); },
    revoke: (id: string) => host.revoke(id),
  };
}

test("pairs once, reconnects after host/phone restarts and foregrounding, then revokes", async () => {
  const f = await fixture(); const profile = await f.pair();
  await until(() => f.states.at(-1)?.status === "authorized", "durable connection authorized");
  expect(f.saved).toEqual(profile);
  expect(JSON.stringify(f.states)).not.toContain(profile.secret);
  expect((await f.active!.call("workspace.list", { include_archived: false })).workspaces[0].title).toBe("Mobile test");
  const count = f.states.filter((state) => state.status === "authorized").length;
  f.restartHost();
  await until(() => f.states.filter((state) => state.status === "authorized").length > count, "host restart reconnects without QR");
  f.manager.setForeground(false); expect(f.active).toBeUndefined();
  f.manager.setForeground(true);
  await until(() => f.states.at(-1)?.status === "authorized", "foreground reconnects");
  await f.restartPhone();
  await until(() => f.states.at(-1)?.status === "authorized", "phone restart restores saved Mac");
  expect(f.methods.filter((method) => method === "remote.approve")).toHaveLength(1);
  await f.revoke(profile.deviceId);
  await until(() => f.saved === null && f.states.at(-1)?.status === "unpaired", "revocation forgets the authenticated Mac");
  expect(f.active).toBeUndefined();
  expect(f.state.devices).toHaveLength(0);
}, 20_000);

test("forgetting while the initial grant is being saved cannot restore a forgotten Mac", async () => {
  let release = () => {}; let saving = false;
  const gate = new Promise<void>((resolve) => { release = resolve; });
  const f = await fixture(async (value) => { if (value) { saving = true; await gate; } });
  await f.pair();
  await until(() => saving, "saving initial grant");
  const forgotten = f.manager.forget(); release(); await forgotten;
  await Bun.sleep(100);
  expect(f.saved).toBeNull(); expect(f.active).toBeUndefined(); expect(f.states.at(-1)?.status).toBe("unpaired");
}, 10_000);

test("reports an incompatible daemon without deleting trust, then recovers after an update", async () => {
  let compatible = false;
  const methods: string[] = [];
  const host = new TrustedHost({ relay: "ws://127.0.0.1:41240", adminToken: randomToken(), connect: async () => ({
    onEvent: () => {}, onClose: () => {}, close: () => {},
    call: async (method) => {
      methods.push(method);
      if (!compatible) throw new Error("unknown variant `remote.state`");
      return { identity_seed: Array.from(new Uint8Array(32).fill(7)), devices: [] };
    },
  }) });
  cleanup.push(() => host.stop()); host.start();
  await until(() => Boolean(host.availabilityError()?.includes("does not support mobile pairing")), "incompatible daemon is reported");
  expect(host.availabilityError()).toContain("does not support mobile pairing");
  expect(methods.every((method) => method === "remote.state")).toBe(true);
  compatible = true;
  await until(() => host.availabilityError() === null, "updated daemon recovers automatically");
  expect(await host.hostIdentity()).toBeDefined();
});


test("a dropped relay connection resumes automatically with saved trust and no new approval", async () => {
  const f = await fixture(); const profile = await f.pair();
  await until(() => f.states.at(-1)?.status === "authorized", "first durable session");
  const before = f.states.filter(state => state.status === "authorized").length;
  f.restartRelay();
  await until(() => f.states.filter(state => state.status === "authorized").length > before, "automatic resume after transport loss");
  expect(f.saved).toEqual(profile);
  expect(f.methods.filter(method => method === "remote.approve")).toHaveLength(1);
  expect((await f.active!.call("workspace.list", { include_archived: false })).workspaces).toHaveLength(1);
}, 15_000);
