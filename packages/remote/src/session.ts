import { JsonChannel } from "./channel";
import { joinRelay } from "./socket";
import { parseInvitation, type Invitation } from "./invitation";
import { parseResume, type ResumeConfig } from "./resume";
import { peerIdFromPrivateKey, securePairing, secureResume, type generateIdentity } from "./secure";
import { RemoteClient } from "./rpc";

export type ConnectionState = {
  status: "unpaired" | "paired" | "connecting" | "reconnecting" | "approval" | "verified" | "authorized";
  error: string | null;
  phonePeer: string | null;
  paired: boolean;
  macPeer: string | null;
};
type Options = {
  identity: () => Promise<Awaited<ReturnType<typeof generateIdentity>>>;
  load: () => Promise<ResumeConfig | null>;
  save: (config: ResumeConfig | null) => Promise<void>;
  onState: (state: ConnectionState) => void;
  onClient: (client?: RemoteClient) => void;
  retryDelay?: (attempt: number) => number;
};

// No Expo dependencies: the same lifecycle is exercised against real Noise in
// tests. Writes are never queued or replayed; each reconnect creates a new client.
export class SessionManager {
  private saved: ResumeConfig | null = null;
  private foreground = true;
  private enabled = true;
  private stopped = false;
  private serial = 0;
  private cancel = () => {};
  private retry?: ReturnType<typeof setTimeout>;
  private attempts = 0;
  private persistence: Promise<void> = Promise.resolve();
  constructor(private options: Options) {}

  async start() {
    const serial = this.serial;
    try {
      const saved = await this.options.load();
      if (this.stopped || serial !== this.serial) return;
      this.saved = saved ? parseResume(saved) : null;
      if (this.saved && this.foreground && this.enabled) this.begin();
      else this.emit(this.saved ? "paired" : "unpaired");
    } catch { if (!this.stopped && serial === this.serial) this.emit("unpaired", "Could not read this Mac's saved connection. Unlock your phone or pair again."); }
  }
  shutdown() { this.stopped = true; this.enabled = false; this.stopAttempt(); }
  disconnect() { this.enabled = false; this.stopAttempt(); this.emit(this.saved ? "paired" : "unpaired"); }
  reconnect() { this.enabled = true; this.attempts = 0; if (this.saved && this.foreground) this.begin(); }
  setForeground(active: boolean) {
    if (active === this.foreground) return;
    this.foreground = active;
    if (!active) { this.stopAttempt(); this.emit(this.saved ? "paired" : "unpaired"); }
    else if (this.saved && this.enabled) this.begin();
  }
  async forget(message?: string) {
    this.enabled = false; this.saved = null; this.stopAttempt();
    try { await this.persist(null); this.emit("unpaired", message ?? null); }
    catch { this.emit("unpaired", "Could not forget the saved Mac. Unlock your phone and try again."); }
  }
  pair(code: string) {
    this.enabled = false;
    try { const invitation = parseInvitation(code); this.begin(invitation); }
    catch { this.stopAttempt(); this.emit(this.saved ? "paired" : "unpaired", "Invalid or expired code. Generate a new QR on your Mac."); }
  }
  private persist(config: ResumeConfig | null) {
    const operation = this.persistence.then(() => this.options.save(config));
    this.persistence = operation.catch(() => {});
    return operation;
  }
  private emit(status: ConnectionState["status"], error: string | null = null, phonePeer: string | null = null) {
    if (!this.stopped) this.options.onState({ status, error, phonePeer, paired: Boolean(this.saved), macPeer: this.saved?.hostPeerId ?? null });
  }
  private stopAttempt() {
    ++this.serial; clearTimeout(this.retry); this.retry = undefined;
    this.cancel(); this.cancel = () => {}; this.options.onClient();
  }
  private begin(invitation?: Invitation) {
    this.stopAttempt();
    if (this.stopped || !this.foreground) return;
    const profile = this.saved;
    if (!invitation && !profile) { this.emit("unpaired"); return; }
    const serial = this.serial;
    const controller = new AbortController();
    let ended = false;
    let close = () => {};
    let client: RemoteClient | undefined;
    let channel: JsonChannel | undefined;
    let phase = "identity";
    let authorized = false;
    const timer = setTimeout(() => controller.abort(), 30_000);
    const current = () => !ended && !this.stopped && serial === this.serial;
    const cancel = () => { ended = true; clearTimeout(timer); controller.abort(); close(); client?.close(); };
    this.cancel = cancel;
    const fail = () => {
      if (!current()) return;
      cancel(); this.options.onClient();
      if (!invitation && this.saved && this.enabled && this.foreground) {
        this.emit("reconnecting", "Connection lost. Reconnecting to your Mac…");
        const delay = this.options.retryDelay?.(this.attempts) ?? Math.min(3000, 500 * 2 ** Math.min(this.attempts, 3)) + Math.floor(Math.random() * 200);
        ++this.attempts;
        this.retry = setTimeout(() => { if (!this.stopped && serial === this.serial && this.enabled && this.foreground) this.begin(); }, delay);
      } else this.emit(this.saved ? "paired" : "unpaired", `Connection ended (${channel?.failure ?? phase}). Generate a fresh Mac code or reconnect.`);
    };
    controller.signal.addEventListener("abort", fail, { once: true });
    this.emit(invitation ? "connecting" : "reconnecting");
    void (async () => {
      try {
        const identity = await this.options.identity();
        if (!current()) return;
        const phonePeer = peerIdFromPrivateKey(identity).toString();
        if (profile && !invitation && profile.phonePeerId !== phonePeer) {
          cancel(); this.enabled = false;
          this.emit("paired", "This phone's identity changed. Forget this Mac and pair again."); return;
        }
        phase = "relay";
        const raw = invitation
          ? await joinRelay(invitation.relay, invitation.room, "phone", invitation.phoneToken, undefined, controller.signal)
          : await joinRelay(profile!.relay, profile!.deviceId, "phone", profile!.phoneToken, undefined, controller.signal, "devices");
        close = () => { void raw.close(); };
        if (!current()) { close(); return; }
        phase = "handshake";
        const secured = invitation ? await securePairing(raw, identity, invitation, "phone", controller.signal) : await secureResume(raw, identity, profile!, "phone", controller.signal);
        close = () => { void secured.connection.close(); };
        if (!current()) { close(); return; }
        phase = "approval";
        channel = new JsonChannel(secured.connection); close = () => channel?.close();
        channel.onclose = fail;
        channel.onmessage = (value) => {
          if (!current() || !value || typeof value !== "object") return;
          const message = value as { type?: string; resume?: unknown };
          if (message.type === "approval_required" && invitation) {
            clearTimeout(timer); this.emit("approval", null, phonePeer);
          } else if (message.type === "verified" && invitation) {
            clearTimeout(timer); this.emit("verified");
          } else if (message.type === "revoked" && !invitation) {
            void this.forget("Your Mac revoked this phone's access. Pair again to request access.");
          } else if (message.type === "authorized" && !authorized) {
            authorized = true; clearTimeout(timer);
            if (invitation) {
              // Older local hosts still offer an explicitly approved disposable
              // session. They cannot create or overwrite a remembered Mac.
              if (message.resume === undefined) {
                client = new RemoteClient(channel!); this.options.onClient(client); this.emit("authorized", null, phonePeer); return;
              }
              void (async () => {
                try {
                  const resume = parseResume(message.resume);
                  if (resume.hostPeerId !== invitation.hostPeerId || resume.phonePeerId !== phonePeer || resume.relay !== invitation.relay) throw new Error("Invalid trusted connection");
                  await this.persist(resume);
                  if (!current()) return;
                  this.saved = resume; this.enabled = true; this.attempts = 0;
                  this.begin();
                } catch { if (current()) { cancel(); this.emit("unpaired", "Could not securely save this Mac. Unlock your phone and pair again."); } }
              })();
            } else {
              this.attempts = 0; client = new RemoteClient(channel!);
              this.options.onClient(client); this.emit("authorized", null, phonePeer);
            }
          } else client?.receive(message);
        };
        channel.send({ type: "check" });
      } catch { fail(); }
    })();
  }
}
