import type { ClientMethods, EventEnvelope, MobileHostMethods } from "@chauffeur/protocol";

/** What the phone may ask: the daemon's methods the mobile host forwards, and its own. */
type PhoneMethods = ClientMethods & MobileHostMethods;
import { JsonChannel } from "./channel";
import { Tunnels } from "./tunnel";

/** An explicit refusal from the Mac, distinct from an unconfirmed transport failure. */
export class RemoteError extends Error {}

export class RemoteClient {
  private closed = false;
  private nextId = 0;
  private pending = new Map<number, { resolve: (value: unknown) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  private listeners = new Set<(event: EventEnvelope) => void>();
  /** Connections to a moved workspace's previews, carried in this session. */
  readonly tunnels: Tunnels;
  constructor(private channel: JsonChannel) { this.tunnels = new Tunnels((message) => channel.send(message)); }
  receive(message: { type?: string; id?: number; result?: unknown; error?: string; event?: { params?: { envelope?: EventEnvelope } } }) {
    if (this.tunnels.receive(message)) return;
    if (message.type === "event") {
      const envelope = message.event?.params?.envelope;
      if (envelope) for (const listener of this.listeners) listener(envelope);
      return;
    }
    if (message.type !== "result") return;
    const entry = this.pending.get(message.id!);
    if (!entry) return;
    this.pending.delete(message.id!); clearTimeout(entry.timer);
    if (message.error) entry.reject(new RemoteError(message.error)); else entry.resolve(message.result);
  }
  call<M extends keyof PhoneMethods>(method: M, params: PhoneMethods[M][0]): Promise<PhoneMethods[M][1]> {
    if (this.closed) return Promise.reject(new Error("Connection ended. Check the conversation before retrying an unconfirmed message."));
    if (this.pending.size >= 8) return Promise.reject(new Error("Too many pending requests"));
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error("No confirmation received. Check the conversation before trying again.")); }, 30_000);
      this.pending.set(id, { resolve: resolve as (value: unknown) => void, reject, timer });
      try { this.channel.send({ type: "rpc", id, method, params }); }
      catch { clearTimeout(timer); this.pending.delete(id); reject(new Error("Connection closed")); }
    });
  }
  onEvent(listener: (event: EventEnvelope) => void) { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; }
  close() {
    this.closed = true;
    this.tunnels.closeAll();
    for (const entry of this.pending.values()) { clearTimeout(entry.timer); entry.reject(new Error("Connection ended before confirmation. Check the conversation before retrying.")); }
    this.pending.clear(); this.listeners.clear();
  }
}
