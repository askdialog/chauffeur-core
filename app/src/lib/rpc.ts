// A typed JSON-RPC client of chauffeurd over its WebSocket.
//
// - `call(method, params)` is typed by `ClientMethods` of the protocol.
// - `subscribe(params, onEvent)` follows the event log. Subscriptions survive reconnects: after a
//   reconnect each one is re-established with `after_seq` = the last persisted `seq` it saw, so
//   no persisted event is lost, and replayed duplicates are dropped.
// - The connection reconnects with exponential backoff and resolves `{url, token}` again before
//   every attempt (the daemon may have restarted on another port).

import {
  PROTOCOL_VERSION,
  type ClientMethods,
  type DaemonNotifications,
  type EventEnvelope,
  type ExtensionId,
  type Replay,
  type RpcError,
  type SubscribeParams,
  type SubscribeResult,
} from "@chauffeur/protocol";

export type Method = keyof ClientMethods;
export type Params<M extends Method> = ClientMethods[M][0];
export type Result<M extends Method> = ClientMethods[M][1];

export type ConnectionInfo = { url: string; token: string };

export type ConnectionState =
  | { status: "connecting"; attempt: number; error?: string }
  /** `shellProfile`: where the user exports a variable for the daemon to get it (`~/.zshenv`). */
  | { status: "open"; daemonVersion: string; shellProfile?: string }
  | { status: "closed" };

/** The subset of the browser WebSocket the client uses; tests provide a fake. */
export interface SocketLike {
  readonly readyState: number;
  send(data: string): void;
  close(code?: number, reason?: string): void;
  onopen: ((event: unknown) => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: ((event: { code?: number; reason?: string }) => void) | null;
  onerror: ((event: unknown) => void) | null;
}

export type ClientOptions = {
  /** Where the daemon is; called before every connection attempt. */
  resolve: () => Promise<ConnectionInfo>;
  createSocket?: (url: string) => SocketLike;
  clientName?: string;
  /** Told of each call the daemon answered with an error. */
  onCallError?: (error: RpcCallError) => void;
  initialBackoffMs?: number;
  maxBackoffMs?: number;
};

/** Error code used when a call could not reach the daemon. */
export const DISCONNECTED = -1;

export class RpcCallError extends Error {
  readonly code: number;
  readonly data: unknown;
  readonly method: string;
  /** The daemon's id of this failure, for an error report. */
  readonly failure?: string;

  constructor(method: string, error: RpcError | { code: number; message: string; data?: unknown }) {
    super(error.message);
    this.name = "RpcCallError";
    this.code = error.code;
    this.data = error.data;
    this.method = method;
    if ("failure" in error && error.failure) this.failure = error.failure;
  }
}

/** True for the errors the daemon returns for methods that other people are still building. */
export function isNotImplemented(error: unknown): boolean {
  return error instanceof RpcCallError && /not implemented/i.test(error.message);
}

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

type PendingCall = {
  method: string;
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
};

type SubscriptionFilter = Omit<SubscribeParams, "after_seq" | "replay">;

export type Subscription = {
  /** Last persisted `seq` delivered to the handler (or the log's head when subscribed live). */
  readonly lastSeq: number | null;
  close(): void;
};

/** Whether the daemon holds a view lease (`extensions.watch`) for this connection right now. */
export type LeaseState = { held: true } | { held: false; error: string | null };

/**
 * A view lease: while held, the daemon keeps the extension's installed version (no update or
 * removal switches it) and sends its events. It is taken again after a reconnect, and retried
 * while the daemon refuses it (the package is starting or not there).
 */
export type Lease = {
  readonly state: LeaseState;
  /** Asks again now instead of at the next retry. */
  retry(): void;
  close(): void;
};

const LEASE_RETRY_MS = [250, 500, 1000, 2000];

class SubscriptionEntry implements Subscription {
  serverId: string | null = null;
  lastSeq: number | null;
  closed = false;
  /** Set for a view lease: subscribed with `extensions.watch` instead of `events.subscribe`. */
  lease: { id: ExtensionId; onState: (state: LeaseState) => void; failures: number; timer: ReturnType<typeof setTimeout> | null } | null = null;

  constructor(
    readonly filter: SubscriptionFilter,
    private readonly afterSeq: number | undefined,
    /** Asked again only while nothing was delivered: a resumption goes on from `lastSeq`, whole. */
    private readonly replay: Replay | undefined,
    readonly onEvent: (envelope: EventEnvelope) => void,
    private readonly onClose: (entry: SubscriptionEntry) => void,
  ) {
    this.lastSeq = afterSeq ?? null;
  }

  /** The part of the history to ask for on (re)subscribing, if any. */
  get replayPart(): Replay | undefined {
    return this.lastSeq === (this.afterSeq ?? null) ? this.replay : undefined;
  }

  /** Delivers an envelope unless it is a replayed duplicate. */
  deliver(envelope: EventEnvelope) {
    if (this.closed) return;
    if (envelope.seq != null) {
      if (this.lastSeq != null && envelope.seq <= this.lastSeq) return;
      this.lastSeq = envelope.seq;
    }
    try {
      this.onEvent(envelope);
    } catch (error) {
      // A handler bug must not take the connection's other subscriptions down with it.
      console.error("event handler failed", error, envelope);
    }
  }

  close() {
    if (this.closed) return;
    this.closed = true;
    if (this.lease?.timer) clearTimeout(this.lease.timer);
    this.onClose(this);
  }
}

const OPEN = 1;

export class DaemonClient {
  private socket: SocketLike | null = null;
  private nextId = 1;
  private pending = new Map<number, PendingCall>();
  /** Calls made before the connection is ready, sent once `hello` succeeds. */
  private queue: Array<{ send: () => void; fail: (error: Error) => void }> = [];
  private subscriptions = new Set<SubscriptionEntry>();
  private byServerId = new Map<string, SubscriptionEntry>();
  /** Notifications for subscriptions whose `events.subscribe` response has not arrived yet. */
  private early = new Map<string, EventEnvelope[]>();
  private ready = false;
  private stopped = false;
  /** Bumped by `stop()`: a connection attempt still resolving when it ran gives up. */
  private generation = 0;
  private attempt = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private listeners = new Set<(state: ConnectionState) => void>();
  private _state: ConnectionState = { status: "connecting", attempt: 0 };
  /** Where the current (or last) connection went. */
  private info: ConnectionInfo | null = null;

  constructor(private readonly options: ClientOptions) {}

  get state(): ConnectionState {
    return this._state;
  }

  /** An HTTP URL on the daemon (images for `<img>` tags), or null before the first connection. */
  httpUrl(path: string): string | null {
    if (!this.info) return null;
    const base = new URL(this.info.url);
    base.protocol = base.protocol === "wss:" ? "https:" : "http:";
    base.pathname = path;
    base.search = `token=${encodeURIComponent(this.info.token)}`;
    return base.toString();
  }

  onStateChange(listener: (state: ConnectionState) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  /** Starts connecting; idempotent. */
  start() {
    if (this.socket || this.retryTimer) return;
    this.stopped = false;
    void this.connect();
  }

  /** Closes the connection for good. */
  stop() {
    this.stopped = true;
    this.generation += 1;
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    this.socket?.close(1000, "client stopped");
    this.socket = null;
    this.teardown("the client was stopped");
    const queued = this.queue;
    this.queue = [];
    for (const call of queued) call.fail(new RpcCallError("", { code: DISCONNECTED, message: "the client was stopped" }));
    this.setState({ status: "closed" });
  }

  call<M extends Method>(method: M, params: Params<M>): Promise<Result<M>> {
    return new Promise<Result<M>>((resolve, reject) => {
      const send = () => {
        const id = this.nextId++;
        this.pending.set(id, {
          method,
          resolve: resolve as (value: unknown) => void,
          reject,
        });
        this.write({ jsonrpc: "2.0", id, method, params });
      };
      if (this.ready) send();
      else this.queue.push({ send, fail: reject });
    });
  }

  /**
   * Follows the event log. With `after_seq`, persisted events after it are replayed first.
   * The handler is called in log order; it survives reconnects until `close()`.
   */
  subscribe(params: SubscribeParams, onEvent: (envelope: EventEnvelope) => void): Subscription {
    const { after_seq, replay, ...filter } = params;
    const entry = new SubscriptionEntry(filter, after_seq, replay, onEvent, (closed) => this.unsubscribe(closed));
    this.subscriptions.add(entry);
    if (this.ready) this.sendSubscribe(entry);
    return entry;
  }

  /**
   * Takes a view lease on an extension (see `Lease`). `onState` is told each time the lease is
   * taken or lost; events of the extension reach `onEvent` while it is held.
   */
  watchExtension(id: ExtensionId, onEvent: (envelope: EventEnvelope) => void, onState: (state: LeaseState) => void): Lease {
    const filter: SubscriptionFilter = { all: false, registry: false, workspaces: [], chats: [], ephemeral: false };
    const entry = new SubscriptionEntry(filter, undefined, undefined, onEvent, (closed) => this.unsubscribe(closed));
    entry.lease = { id, onState, failures: 0, timer: null };
    let state: LeaseState = { held: false, error: null };
    const lease = entry.lease;
    lease.onState = (next) => {
      state = next;
      onState(next);
    };
    this.subscriptions.add(entry);
    if (this.ready) this.sendSubscribe(entry);
    return {
      get state() {
        return state;
      },
      retry: () => {
        if (entry.closed || entry.serverId || !this.ready || !lease.timer) return;
        clearTimeout(lease.timer);
        lease.timer = null;
        this.sendSubscribe(entry);
      },
      close: () => entry.close(),
    };
  }

  private unsubscribe(entry: SubscriptionEntry) {
    this.subscriptions.delete(entry);
    if (entry.serverId) {
      this.byServerId.delete(entry.serverId);
      if (this.ready) {
        this.call("events.unsubscribe", { subscription: entry.serverId }).catch(() => {});
      }
    }
  }

  private sendSubscribe(entry: SubscriptionEntry) {
    if (entry.lease) {
      this.sendWatch(entry, entry.lease);
      return;
    }
    const params: SubscribeParams = { ...entry.filter };
    if (entry.lastSeq != null) params.after_seq = entry.lastSeq;
    const replay = entry.replayPart;
    if (replay) params.replay = replay;
    entry.serverId = null;
    this.call("events.subscribe", params).then(
      (result: SubscribeResult) => {
        if (entry.closed) {
          this.call("events.unsubscribe", { subscription: result.subscription }).catch(() => {});
          return;
        }
        entry.serverId = result.subscription;
        this.byServerId.set(result.subscription, entry);
        // A live-only subscription resumes from the head it started at.
        if (entry.lastSeq == null) entry.lastSeq = result.last_seq;
        const buffered = this.early.get(result.subscription);
        this.early.delete(result.subscription);
        for (const envelope of buffered ?? []) entry.deliver(envelope);
      },
      () => {
        // The connection dropped: the reconnect re-subscribes. A rejected filter (unknown
        // workspace) is not retried.
      },
    );
  }

  private sendWatch(entry: SubscriptionEntry, lease: NonNullable<SubscriptionEntry["lease"]>) {
    entry.serverId = null;
    if (lease.timer) clearTimeout(lease.timer);
    lease.timer = null;
    this.call("extensions.watch", { id: lease.id }).then(
      (result: SubscribeResult) => {
        if (entry.closed) {
          this.call("events.unsubscribe", { subscription: result.subscription }).catch(() => {});
          return;
        }
        entry.serverId = result.subscription;
        this.byServerId.set(result.subscription, entry);
        lease.failures = 0;
        lease.onState({ held: true });
        const buffered = this.early.get(result.subscription);
        this.early.delete(result.subscription);
        for (const envelope of buffered ?? []) entry.deliver(envelope);
      },
      (error: unknown) => {
        if (entry.closed) return;
        // A dropped connection takes the lease again once it is back (`teardown` said so).
        if (error instanceof RpcCallError && error.code === DISCONNECTED) return;
        lease.onState({ held: false, error: errorMessage(error) });
        const delay = LEASE_RETRY_MS[Math.min(lease.failures, LEASE_RETRY_MS.length - 1)]!;
        lease.failures += 1;
        lease.timer = setTimeout(() => {
          lease.timer = null;
          if (!entry.closed && this.ready && !entry.serverId) this.sendWatch(entry, lease);
        }, delay);
      },
    );
  }

  private setState(state: ConnectionState) {
    this._state = state;
    for (const listener of this.listeners) listener(state);
  }

  private write(message: unknown) {
    const socket = this.socket;
    if (!socket || socket.readyState !== OPEN) return;
    socket.send(JSON.stringify(message));
  }

  private async connect() {
    if (this.stopped) return;
    const generation = this.generation;
    this.attempt += 1;
    const lastError = this._state.status === "connecting" ? this._state.error : undefined;
    this.setState({ status: "connecting", attempt: this.attempt, error: lastError });
    let info: ConnectionInfo;
    try {
      info = await this.options.resolve();
    } catch (error) {
      if (generation === this.generation) this.scheduleRetry(errorMessage(error));
      return;
    }
    // Stopped meanwhile, and maybe started again: that start makes its own connection.
    if (this.stopped || generation !== this.generation) return;
    this.info = info;
    const url = `${info.url}${info.url.includes("?") ? "&" : "?"}token=${encodeURIComponent(info.token)}`;
    let socket: SocketLike;
    try {
      socket = (this.options.createSocket ?? ((u) => new WebSocket(u) as unknown as SocketLike))(url);
    } catch (error) {
      this.scheduleRetry(errorMessage(error));
      return;
    }
    this.socket = socket;
    socket.onopen = () => {
      void this.handshake(socket);
    };
    socket.onmessage = (event) => {
      if (this.socket !== socket) return;
      this.receive(typeof event.data === "string" ? event.data : String(event.data));
    };
    socket.onerror = () => {};
    socket.onclose = (event) => {
      if (this.socket !== socket) return;
      this.socket = null;
      this.teardown("the connection to the daemon was lost");
      this.scheduleRetry(event.reason || "the daemon closed the connection");
    };
  }

  private async handshake(socket: SocketLike) {
    // `hello` goes out before anything queued.
    const id = this.nextId++;
    const hello = new Promise<Result<"hello">>((resolve, reject) => {
      this.pending.set(id, { method: "hello", resolve: resolve as (value: unknown) => void, reject });
    });
    this.write({
      jsonrpc: "2.0",
      id,
      method: "hello",
      params: { protocol_version: PROTOCOL_VERSION, client: this.options.clientName ?? "chauffeur-app" },
    });
    try {
      const result = await hello;
      if (this.socket !== socket) return;
      this.ready = true;
      this.attempt = 0;
      this.setState({
        status: "open",
        daemonVersion: result.daemon_version,
        ...(result.shell_profile ? { shellProfile: result.shell_profile } : {}),
      });
      for (const entry of this.subscriptions) this.sendSubscribe(entry);
      const queued = this.queue;
      this.queue = [];
      for (const call of queued) call.send();
    } catch (error) {
      // A protocol mismatch: keep retrying slowly, the error is shown.
      socket.close(1000, errorMessage(error));
    }
  }

  private receive(text: string) {
    let message: {
      id?: number | string | null;
      method?: string;
      params?: unknown;
      result?: unknown;
      error?: RpcError;
    };
    try {
      message = JSON.parse(text);
    } catch {
      return;
    }
    if (message.method === "event") {
      const notification = message.params as DaemonNotifications["event"];
      if (!notification?.envelope) return;
      const entry = this.byServerId.get(notification.subscription);
      if (entry) entry.deliver(notification.envelope);
      else {
        const buffered = this.early.get(notification.subscription) ?? [];
        buffered.push(notification.envelope);
        this.early.set(notification.subscription, buffered);
      }
      return;
    }
    if (message.id == null) return;
    const id = typeof message.id === "number" ? message.id : Number(message.id);
    const pending = this.pending.get(id);
    if (!pending) return;
    this.pending.delete(id);
    if (message.error) {
      const error = new RpcCallError(pending.method, message.error);
      this.options.onCallError?.(error);
      pending.reject(error);
    } else pending.resolve(message.result ?? null);
  }

  /** Fails in-flight calls; subscriptions stay registered for the next connection. */
  private teardown(reason: string) {
    this.ready = false;
    this.byServerId.clear();
    this.early.clear();
    for (const entry of this.subscriptions) {
      entry.serverId = null;
      if (entry.lease) {
        if (entry.lease.timer) clearTimeout(entry.lease.timer);
        entry.lease.timer = null;
        // The daemon dropped the lease with the connection: the version may change meanwhile.
        entry.lease.onState({ held: false, error: null });
      }
    }
    const pending = [...this.pending.values()];
    this.pending.clear();
    for (const call of pending) {
      call.reject(new RpcCallError(call.method, { code: DISCONNECTED, message: reason }));
    }
  }

  private scheduleRetry(error: string) {
    if (this.stopped) return;
    const initial = this.options.initialBackoffMs ?? 250;
    const max = this.options.maxBackoffMs ?? 5000;
    const delay = Math.min(max, initial * 2 ** Math.max(0, this.attempt - 1));
    this.setState({ status: "connecting", attempt: this.attempt, error });
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.connect();
    }, delay);
  }
}
