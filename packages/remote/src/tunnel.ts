// Byte tunnels inside the encrypted channel (docs/workspace-relay.md, "Previews on the phone"):
// the phone's web view reaches a moved workspace's declared preview through connections the phone
// accepts on its loopback, each carried as tunnel messages to the Mac's mobile host, which passes
// it to the daemon. Bytes travel base64-encoded, as every message of the channel is JSON. Each
// side keeps at most TUNNEL_WINDOW bytes of a tunnel unacknowledged, pausing its source until the
// other side wrote them: a slow reader never fills the channel's bounded queue, which would close
// the whole session.
import { fromString, toString } from "uint8arrays";

/** Bytes per message: a 16 KiB chunk is about 22 KiB of JSON. */
export const TUNNEL_CHUNK = 16 * 1024;
/** Bytes of one tunnel sent and not yet written by the other side. */
export const TUNNEL_WINDOW = 64 * 1024;
/** Tunnels open at a time in one session: a page opens about six per origin. Their windows
 * together stay well under the channel's 2 MiB queue. */
export const MAX_TUNNELS = 12;
/** A preview's place in the workspace's port block, bounded as the daemon bounds it. */
export const MAX_PREVIEW_OFFSET = 1024;

/** One side's local end of a tunnel: a socket of the phone's loopback, or the Mac daemon's
 * WebSocket of the connection. */
export interface TunnelEnd {
  /** Writes bytes the other side sent; `written` once they are taken. */
  write(bytes: Uint8Array, written: () => void): void;
  /** The other side ended its writes: end ours, keep reading. */
  end(): void;
  destroy(): void;
  pause(): void;
  resume(): void;
}

/** What the local end reports, for the tunnel to pass on. */
export interface TunnelHandle {
  data(bytes: Uint8Array): void;
  /** The local end ended its writes. */
  ended(): void;
  /** The local end is gone; `error` says why when it never opened. */
  closed(error?: string): void;
}

type Accept = (workspace: string, offset: number, handle: TunnelHandle) => Promise<TunnelEnd>;

type Message = { type?: unknown; tunnel?: unknown; workspace?: unknown; offset?: unknown; data?: unknown; bytes?: unknown; error?: unknown };

class Tunnel implements TunnelHandle {
  private local?: TunnelEnd;
  private early: (Uint8Array | "end")[] = [];
  private unacknowledged = 0;
  /** Read from the local end, not sent yet: past the window. */
  private outgoing: (Uint8Array | "end")[] = [];
  private paused = false;
  private gone = false;
  constructor(readonly id: number, private owner: Tunnels) {}

  attach(local: TunnelEnd) {
    if (this.gone) { local.destroy(); return; }
    this.local = local;
    for (const item of this.early.splice(0)) this.deliver(item);
  }
  data(bytes: Uint8Array) {
    if (this.gone) return;
    for (let at = 0; at < bytes.length; at += TUNNEL_CHUNK) this.outgoing.push(bytes.slice(at, at + TUNNEL_CHUNK));
    this.flush();
  }
  ended() { if (!this.gone) { this.outgoing.push("end"); this.flush(); } }
  /** Sends what the window allows; the local end waits, paused, for the rest to be taken. */
  private flush() {
    while (this.outgoing.length && this.unacknowledged < TUNNEL_WINDOW && !this.gone) {
      const item = this.outgoing.shift()!;
      if (item === "end") { this.owner.post({ type: "tunnel_end", tunnel: this.id }); continue; }
      this.unacknowledged += item.length;
      this.owner.post({ type: "tunnel_data", tunnel: this.id, data: toString(item, "base64pad") });
    }
    const full = this.outgoing.length > 0 || this.unacknowledged >= TUNNEL_WINDOW;
    if (full && !this.paused) { this.paused = true; this.local?.pause(); }
    else if (!full && this.paused && this.unacknowledged < TUNNEL_WINDOW / 2) { this.paused = false; this.local?.resume(); }
  }
  closed(error?: string) {
    if (this.gone) return;
    this.forget();
    this.owner.post({ type: "tunnel_close", tunnel: this.id, ...(error ? { error } : {}) });
  }

  receive(message: Message) {
    if (message.type === "tunnel_data" && typeof message.data === "string" && message.data.length <= TUNNEL_CHUNK * 2) {
      let bytes: Uint8Array;
      try { bytes = fromString(message.data, "base64pad"); }
      catch { this.closed("invalid tunnel data"); this.local?.destroy(); return; }
      this.deliver(bytes);
    } else if (message.type === "tunnel_ack" && Number.isSafeInteger(message.bytes) && Number(message.bytes) > 0) {
      this.unacknowledged = Math.max(0, this.unacknowledged - Number(message.bytes));
      this.flush();
    } else if (message.type === "tunnel_end") this.deliver("end");
    else if (message.type === "tunnel_close") { this.forget(); this.local?.destroy(); }
    else { this.closed("invalid tunnel message"); this.local?.destroy(); }
  }
  destroy() { this.forget(); this.local?.destroy(); }

  private deliver(item: Uint8Array | "end") {
    if (this.gone) return;
    if (!this.local) { this.early.push(item); return; }
    if (item === "end") { this.local.end(); return; }
    const length = item.length;
    this.local.write(item, () => { if (!this.gone) this.owner.post({ type: "tunnel_ack", tunnel: this.id, bytes: length }); });
  }
  private forget() { this.gone = true; this.early = []; this.outgoing = []; this.owner.forget(this.id); }
}

/** The tunnels of one session. The phone opens them (`open`); the Mac host accepts them
 * (`accept`, given), each tunnel's local end being then the daemon's connection. */
export class Tunnels {
  private tunnels = new Map<number, Tunnel>();
  private next = 0;
  constructor(private send: (message: unknown) => void, private accept?: Accept) {}

  /** Opens a tunnel to `workspace`'s preview at `offset`, whose local end is `local`. */
  open(workspace: string, offset: number, local: TunnelEnd): TunnelHandle {
    const tunnel = new Tunnel(++this.next, this);
    this.tunnels.set(tunnel.id, tunnel);
    tunnel.attach(local);
    this.post({ type: "tunnel_open", tunnel: tunnel.id, workspace, offset });
    return tunnel;
  }

  /** Takes a message of the channel; false when it is no tunnel's. */
  receive(value: unknown): boolean {
    const message = value as Message | null;
    if (typeof message?.type !== "string" || !message.type.startsWith("tunnel_")) return false;
    if (!Number.isSafeInteger(message.tunnel) || Number(message.tunnel) < 1) return true;
    const id = Number(message.tunnel);
    if (message.type === "tunnel_open") { this.opened(id, message); return true; }
    // A tunnel this side closed meanwhile: what crossed it is dropped.
    this.tunnels.get(id)?.receive(message);
    return true;
  }

  /** Ends every tunnel, as the session ends. */
  closeAll() { for (const tunnel of [...this.tunnels.values()]) tunnel.destroy(); }
  get size() { return this.tunnels.size; }

  post(message: unknown) { try { this.send(message); } catch { this.closeAll(); } }
  forget(id: number) { this.tunnels.delete(id); }

  private opened(id: number, message: Message) {
    const refuse = (error: string) => this.post({ type: "tunnel_close", tunnel: id, error });
    if (!this.accept) return refuse("This side opens no tunnel");
    if (this.tunnels.has(id)) return refuse("Tunnel already open");
    if (this.tunnels.size >= MAX_TUNNELS) return refuse("Too many preview connections at once");
    const offset = Number(message.offset);
    if (typeof message.workspace !== "string" || !/^[A-Za-z0-9_.-]{1,200}$/.test(message.workspace) ||
        !Number.isInteger(offset) || offset < 0 || offset >= MAX_PREVIEW_OFFSET) return refuse("Invalid preview");
    const tunnel = new Tunnel(id, this);
    this.tunnels.set(id, tunnel);
    this.accept(message.workspace, offset, tunnel).then(
      (local) => tunnel.attach(local),
      (error: unknown) => tunnel.closed(error instanceof Error ? error.message : "Preview unavailable"),
    );
  }
}
