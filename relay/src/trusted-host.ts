import type { RemoteDevice, RemoteDeviceInfo, RemoteStateResult } from "@chauffeur/protocol";
import { identityFromSeed, peerIdFromPrivateKey, parseResume, relayUrl, joinRelay, secureResume, JsonChannel, type ResumeConfig } from "@chauffeur/remote";
import { connectDaemon, type DaemonConnection } from "./daemon";
import { PushObserver } from "./push-observer";
import { createBridge } from "./bridge";

type Options = { relay: string; adminToken: string; connect?: () => Promise<DaemonConnection> };
type Slot = { stop: (reason?: "revoked") => Promise<void>; device: RemoteDevice; connected: boolean };

function pause(ms: number, signal: AbortSignal) {
  return new Promise<void>((resolve) => {
    const done = () => { clearTimeout(timer); signal.removeEventListener("abort", done); resolve(); };
    const timer = setTimeout(done, ms);
    signal.addEventListener("abort", done, { once: true });
    if (signal.aborted) done();
  });
}

// Private local companion. Only chauffeurd persists identities and device grants.
// This module must never be included in the deployed opaque relay entry point.
export class TrustedHost {
  private control?: DaemonConnection;
  private push?: PushObserver;
  private pushStarted = false;
  private identity?: Parameters<typeof peerIdFromPrivateKey>[0];
  private devices: RemoteDevice[] = [];
  private slots = new Map<string, Slot>();
  private stopped = false;
  private timer?: ReturnType<typeof setTimeout>;
  private refreshing?: Promise<void>;
  private unavailable: string | null = "Connecting to the Chauffeur daemon…";
  constructor(private options: Options) { relayUrl(options.relay); }

  start() {
    const poll = async () => {
      try { await this.refresh(); this.unavailable = null; }
      catch (error) {
        const incompatible = error instanceof Error && /unknown variant [`']remote\.state|method not found/i.test(error.message);
        this.unavailable = incompatible
          ? "This Chauffeur daemon does not support mobile pairing. Install the mobile-enabled build; your saved pairing has not been erased."
          : "Cannot reach the Chauffeur daemon. Keep Chauffeur running on this Mac.";
      }
      if (!this.stopped) this.timer = setTimeout(() => void poll(), 1000);
    };
    void poll();
  }
  stop() {
    this.stopped = true; clearTimeout(this.timer);
    for (const slot of this.slots.values()) slot.stop();
    this.push?.stop(); this.slots.clear(); this.control?.close(); this.control = undefined;
  }
  publicDevices(): (RemoteDeviceInfo & { connected: boolean })[] {
    return this.devices.map((device) => ({ ...device.info, connected: this.slots.get(device.info.id)?.connected ?? false }));
  }
  availabilityError() { return this.unavailable; }
  async hostIdentity() { await this.refresh(); if (!this.identity) throw new Error("Update or start the Chauffeur daemon"); return this.identity; }
  async approve(peer: string, name: string): Promise<ResumeConfig> {
    await this.refresh();
    const device = await this.control!.call("remote.approve", { peer_id: peer, name, relay: this.options.relay }) as RemoteDevice;
    await this.register(device);
    await this.refresh();
    return this.profile(device);
  }
  async revoke(id: string) {
    await this.refresh();
    await this.control!.call("remote.revoke", { device_id: id });
    await this.slots.get(id)?.stop("revoked"); this.slots.delete(id);
    await this.registerRequest(`/v1/devices/${encodeURIComponent(id)}`, "DELETE").catch(() => {});
    await this.refresh();
  }
  private profile(device: RemoteDevice) {
    return parseResume({ version: 1, relay: device.info.relay, deviceId: device.info.id, phoneToken: device.phone_token, hostPeerId: peerIdFromPrivateKey(this.identity!).toString(), phonePeerId: device.info.peer_id, secret: device.secret });
  }
  private async registerRequest(path: string, method: string, body?: unknown) {
    const url = new URL(this.options.relay); url.protocol = url.protocol === "wss:" ? "https:" : "http:"; url.pathname = path;
    const response = await fetch(url, { method, headers: { Authorization: `Bearer ${this.options.adminToken}`, "Content-Type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(5000) });
    if (!response.ok) throw new Error("Relay unavailable");
  }
  private register(device: RemoteDevice) {
    return this.registerRequest("/v1/devices", "POST", { deviceId: device.info.id, hostToken: device.host_token, phoneToken: device.phone_token });
  }
  private refresh() {
    if (this.refreshing) return this.refreshing;
    this.refreshing = this.readState().finally(() => { this.refreshing = undefined; });
    return this.refreshing;
  }
  private async readState() {
    if (this.stopped) throw new Error("Host stopped");
    if (!this.control) {
      const connection = await (this.options.connect ?? connectDaemon)();
      if (this.stopped) { connection.close(); return; }
      this.control = connection;
      this.push = new PushObserver(connection, () => this.devices, id => this.slots.get(id)?.connected ?? false);
      connection.onClose = () => {
        if (this.control !== connection) return;
        this.control = undefined; this.push?.stop(); this.pushStarted = false;
        for (const slot of this.slots.values()) slot.stop(); this.slots.clear();
      };
    }
    const state = await this.control.call("remote.state", {}) as RemoteStateResult;
    const identity = await identityFromSeed("Ed25519", Uint8Array.from(state.identity_seed));
    if (this.stopped) return;
    if (this.identity && peerIdFromPrivateKey(identity).toString() !== peerIdFromPrivateKey(this.identity).toString()) {
      for (const slot of this.slots.values()) slot.stop(); this.slots.clear();
    }
    this.identity = identity; this.devices = state.devices;
    if (!this.pushStarted && this.push) {
      try { await this.push.start(); this.pushStarted = true; }
      catch(error) { this.control?.close(); throw error; }
    }
    for (const [id, slot] of this.slots) if (!state.devices.some((device) => device.info.id === id)) {
      await slot.stop("revoked"); this.slots.delete(id);
      void this.registerRequest(`/v1/devices/${encodeURIComponent(id)}`, "DELETE").catch(() => {});
    }
    for (const device of state.devices) if (device.info.relay === this.options.relay && !this.slots.has(device.info.id)) this.follow(device);
  }
  private follow(device: RemoteDevice) {
    const controller = new AbortController();
    let close = () => {};
    let notifyRevoked: (() => void) | undefined;
    const slot: Slot = { device, connected: false, stop: async (reason) => {
      // Aborting joinRelay closes the transport. Give the authenticated notice
      // a bounded delivery window before aborting or deleting the relay slot.
      // The daemon has already revoked access, so RPC checks reject throughout.
      if (reason === "revoked" && notifyRevoked) {
        notifyRevoked();
        await new Promise((resolve) => setTimeout(resolve, 100));
      }
      controller.abort();
      close();
    } };
    this.slots.set(device.info.id, slot);
    const profile = this.profile(device);
    let failures = 0;
    const renew = setInterval(() => { void this.register(device).catch(() => {}); }, 10 * 60_000);
    void (async () => {
      try {
        while (!controller.signal.aborted) {
          try {
            await this.register(device);
            const signal = AbortSignal.any([controller.signal, AbortSignal.timeout(60_000)]);
            const raw = await joinRelay(profile.relay, profile.deviceId, "host", device.host_token, undefined, signal, "devices");
            close = () => { void raw.close(); };
            if (controller.signal.aborted) { close(); break; }
            const secured = await secureResume(raw, this.identity!, profile, "host", AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]));
            const channel = new JsonChannel(secured.connection);
            notifyRevoked = () => { try { channel.send({ type: "revoked" }); } catch {} };
            let finish = () => {};
            const ended = new Promise<void>((resolve) => { finish = resolve; });
            let session: ReturnType<typeof createBridge>;
            const check = async (connection: DaemonConnection) => {
              const info = await connection.call("remote.check", { device_id: device.info.id }) as RemoteDeviceInfo;
              if (info.peer_id !== secured.remotePeer.toString()) throw new Error("Device identity changed");
            };
            let daemon: DaemonConnection | undefined;
            session = createBridge(channel, async () => {
              const connection = await (this.options.connect ?? connectDaemon)();
              try { await check(connection); daemon = connection; return connection; }
              catch (error) { connection.close(); throw error; }
            }, () => { slot.connected = false; finish(); }, {
              onGreeting: async () => { await session.approve(); slot.connected = true; failures = 0; },
              beforeCall: async () => { if (!daemon) throw new Error("Not authorized"); await check(daemon); },
              pushCall: async (method, config) => {
                if (!daemon) throw new Error("Not authorized");
                if (method === "push.status") return daemon.call("remote.push_get", { device_id: device.info.id });
                if (!config || typeof config !== "object") throw new Error("Invalid notification configuration");
                return daemon.call("remote.push_set", { device_id: device.info.id, config });
              },
            });
            close = () => session.close();
            if (controller.signal.aborted) close();
            await ended;
          } catch { close(); }
          slot.connected = false;
          notifyRevoked = undefined;
          if (!controller.signal.aborted) await pause(Math.min(15_000, 1000 * 2 ** Math.min(failures++, 4)), controller.signal);
        }
      } finally { clearInterval(renew); close(); }
    })();
  }
}
