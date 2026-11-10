import { describe, expect, test } from "bun:test";
import { PROTOCOL_VERSION, type EventEnvelope } from "@chauffeur/protocol";
import { DaemonClient, DISCONNECTED, RpcCallError, type SocketLike } from "./rpc";

/** An in-memory WebSocket: the test plays the daemon. */
class FakeSocket implements SocketLike {
  readyState = 0;
  sent: Array<{ id?: number; method?: string; params?: Record<string, unknown> }> = [];
  onopen: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code?: number; reason?: string }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;

  constructor(readonly url: string) {}

  send(data: string) {
    this.sent.push(JSON.parse(data));
  }
  close() {
    if (this.readyState === 3) return;
    this.readyState = 3;
    this.onclose?.({ code: 1000, reason: "" });
  }
  // --- daemon side ---
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  push(message: unknown) {
    this.onmessage?.({ data: JSON.stringify(message) });
  }
  reply(id: number | undefined, result: unknown) {
    this.push({ jsonrpc: "2.0", id, result });
  }
  fail(id: number | undefined, code: number, message: string) {
    this.push({ jsonrpc: "2.0", id, error: { code, message } });
  }
  event(subscription: string, envelope: EventEnvelope) {
    this.push({ jsonrpc: "2.0", method: "event", params: { subscription, envelope } });
  }
  drop() {
    this.readyState = 3;
    this.onclose?.({ code: 1006, reason: "gone" });
  }
  last(method: string) {
    const found = [...this.sent].reverse().find((m) => m.method === method);
    if (!found) throw new Error(`no ${method} sent; sent: ${this.sent.map((m) => m.method).join(", ")}`);
    return found;
  }
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));
const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function harness() {
  const sockets: FakeSocket[] = [];
  let port = 1000;
  const client = new DaemonClient({
    resolve: async () => ({ url: `ws://127.0.0.1:${++port}/ws`, token: "s3cret" }),
    createSocket: (url) => {
      const socket = new FakeSocket(url);
      sockets.push(socket);
      return socket;
    },
    initialBackoffMs: 1,
    maxBackoffMs: 5,
  });
  /** Opens the latest socket and answers `hello`. */
  const connect = async () => {
    await tick();
    const socket = sockets[sockets.length - 1];
    socket.open();
    const hello = socket.last("hello");
    socket.reply(hello.id, { protocol_version: 1, daemon_version: "0.1.0", home: "/h", workspaces_dir: "/w" });
    await tick();
    return socket;
  };
  return { client, sockets, connect };
}

function envelope(seq: number | null, message = `m${seq}`): EventEnvelope {
  return { seq, ts: "2026-10-13T00:00:00Z", event: { scope: "workspace", workspace_id: "w", event: { type: "progress", message } } };
}

describe("DaemonClient", () => {
  test("authenticates with the token, says hello first, then sends queued calls", async () => {
    const { client, sockets, connect } = harness();
    client.start();
    const pending = client.call("workspace.list", { include_archived: true });
    const socket = await connect();
    expect(socket.url).toBe("ws://127.0.0.1:1001/ws?token=s3cret");
    expect(socket.sent[0].method).toBe("hello");
    expect(socket.sent[0].params).toMatchObject({ protocol_version: PROTOCOL_VERSION });
    const list = socket.last("workspace.list");
    expect(list.params).toEqual({ include_archived: true });
    socket.reply(list.id, { workspaces: [], sections: [] });
    expect(await pending).toEqual({ workspaces: [], sections: [] });
    expect(client.state).toEqual({ status: "open", daemonVersion: "0.1.0" });
    expect(sockets).toHaveLength(1);
    client.stop();
  });

  test("errors become RpcCallError with the daemon's code and message", async () => {
    const { client, connect } = harness();
    client.start();
    const socket = await connect();
    const call = client.call("diff.get", { workspace: "w", stat_only: false });
    await tick();
    socket.fail(socket.last("diff.get").id, 503, "not implemented yet");
    const error = await call.catch((e) => e);
    expect(error).toBeInstanceOf(RpcCallError);
    expect(error.code).toBe(503);
    expect(error.message).toBe("not implemented yet");
    expect(error.method).toBe("diff.get");
    client.stop();
  });

  test("notifications that arrive before the subscribe response are delivered after it, in order", async () => {
    const { client, connect } = harness();
    client.start();
    const socket = await connect();
    const seen: Array<number | null> = [];
    client.subscribe({ all: false, registry: false, workspaces: ["w"], chats: [], ephemeral: true, after_seq: 0 }, (env) =>
      seen.push(env.seq),
    );
    await tick();
    const subscribe = socket.last("events.subscribe");
    expect(subscribe.params).toMatchObject({ workspaces: ["w"], after_seq: 0, ephemeral: true });
    // The daemon starts replaying before it answers.
    socket.event("sub-1", envelope(1));
    socket.event("sub-1", envelope(2));
    expect(seen).toEqual([]);
    socket.reply(subscribe.id, { subscription: "sub-1", last_seq: 2 });
    await tick();
    expect(seen).toEqual([1, 2]);
    socket.event("sub-1", envelope(null));
    socket.event("sub-1", envelope(3));
    // Unknown subscriptions do not leak into this one.
    socket.event("sub-other", envelope(4));
    expect(seen).toEqual([1, 2, null, 3]);
    client.stop();
  });

  test("a part of the history is asked for until something arrives, then a reconnection resumes whole", async () => {
    const { client, connect, sockets } = harness();
    client.start();
    let socket = await connect();
    client.subscribe(
      { all: false, registry: false, workspaces: ["w"], chats: [], ephemeral: true, after_seq: 0, replay: { part: "tail", per_chat: 50 } },
      () => {},
    );
    await tick();
    expect(socket.last("events.subscribe").params).toMatchObject({ after_seq: 0, replay: { part: "tail", per_chat: 50 } });
    // Dropped before anything came: asked again the same way.
    socket.drop();
    await wait(10);
    socket = await connect();
    const again = socket.last("events.subscribe");
    expect(again.params).toMatchObject({ after_seq: 0, replay: { part: "tail", per_chat: 50 } });
    socket.reply(again.id, { subscription: "t", last_seq: 90 });
    await tick();
    socket.event("t", envelope(40));
    socket.drop();
    await wait(10);
    socket = await connect();
    const resumed = socket.last("events.subscribe");
    expect(resumed.params).toMatchObject({ after_seq: 40 });
    expect(resumed.params?.replay).toBeUndefined();
    expect(sockets).toHaveLength(3);
    client.stop();
  });

  test("after a reconnect, subscriptions resume from the last seen seq and duplicates are dropped", async () => {
    const { client, sockets, connect } = harness();
    const states: string[] = [];
    client.onStateChange((s) => states.push(s.status));
    client.start();
    let socket = await connect();
    const seen: Array<string> = [];
    client.subscribe({ all: false, registry: true, workspaces: [], chats: [], ephemeral: false }, (env) =>
      seen.push(env.event.scope === "workspace" && env.event.event.type === "progress" ? env.event.event.message : "?"),
    );
    await tick();
    const first = socket.last("events.subscribe");
    // A live-only subscription: no after_seq on the first subscribe.
    expect(first.params?.after_seq).toBeUndefined();
    socket.reply(first.id, { subscription: "a", last_seq: 10 });
    await tick();
    socket.event("a", envelope(11, "eleven"));
    socket.event("a", envelope(12, "twelve"));

    const inflight = client.call("repos.list", { refresh: false });
    await tick();
    socket.drop();
    const error = await inflight.catch((e) => e);
    expect(error).toBeInstanceOf(RpcCallError);
    expect(error.code).toBe(DISCONNECTED);

    await wait(10);
    expect(sockets).toHaveLength(2);
    // The address is resolved again: the daemon may have moved.
    expect(sockets[1].url).toBe("ws://127.0.0.1:1002/ws?token=s3cret");
    socket = await connect();
    const again = socket.last("events.subscribe");
    expect(again.params).toMatchObject({ registry: true, after_seq: 12 });
    socket.event("b", envelope(12, "twelve (replayed)"));
    socket.event("b", envelope(13, "thirteen"));
    socket.reply(again.id, { subscription: "b", last_seq: 13 });
    await tick();
    expect(seen).toEqual(["eleven", "twelve", "thirteen"]);
    expect(states).toContain("connecting");
    expect(states[states.length - 1]).toBe("open");
    client.stop();
  });

  test("a subscription closed before its response is unsubscribed on the daemon", async () => {
    const { client, connect } = harness();
    client.start();
    const socket = await connect();
    const seen: unknown[] = [];
    const sub = client.subscribe({ all: true, registry: false, workspaces: [], chats: [], ephemeral: false }, (env) => seen.push(env));
    await tick();
    const subscribe = socket.last("events.subscribe");
    sub.close();
    socket.reply(subscribe.id, { subscription: "late", last_seq: 0 });
    await tick();
    expect(socket.last("events.unsubscribe").params).toEqual({ subscription: "late" });
    socket.event("late", envelope(1));
    expect(seen).toHaveLength(0);
    client.stop();
  });

  test("a failed resolve is retried with backoff and reported", async () => {
    let attempts = 0;
    const sockets: FakeSocket[] = [];
    const client = new DaemonClient({
      resolve: async () => {
        attempts += 1;
        if (attempts < 3) throw new Error("no daemon.json");
        return { url: "ws://127.0.0.1:9/ws", token: "t" };
      },
      createSocket: (url) => {
        const socket = new FakeSocket(url);
        sockets.push(socket);
        return socket;
      },
      initialBackoffMs: 1,
      maxBackoffMs: 2,
    });
    const errors: string[] = [];
    client.onStateChange((s) => {
      if (s.status === "connecting" && s.error) errors.push(s.error);
    });
    client.start();
    await wait(30);
    expect(attempts).toBe(3);
    expect(sockets).toHaveLength(1);
    expect(errors).toContain("no daemon.json");
    client.stop();
    expect(client.state.status).toBe("closed");
  });

  test("calls queued while disconnected fail when the client stops", async () => {
    const { client } = harness();
    const call = client.call("workspace.get", { workspace: "x" });
    client.stop();
    const error = await call.catch((e) => e);
    expect(error).toBeInstanceOf(RpcCallError);
    expect(error.code).toBe(DISCONNECTED);
  });

  test("stopped and started again while connecting, it opens one socket", async () => {
    const { client, sockets, connect } = harness();
    client.start();
    client.stop();
    client.start();
    await connect();
    await tick();
    expect(sockets).toHaveLength(1);
    expect(client.state.status).toBe("open");
    client.stop();
  });
});

describe("extension view leases", () => {
  test("a lease is held once the daemon accepts extensions.watch, and its events reach the view", async () => {
    const { client, connect } = harness();
    client.start();
    const socket = await connect();
    const states: unknown[] = [];
    const events: EventEnvelope[] = [];
    const lease = client.watchExtension("database", (envelope) => events.push(envelope), (state) => states.push(state));
    await tick();
    const watch = socket.last("extensions.watch");
    expect(watch.params).toEqual({ id: "database" });
    socket.reply(watch.id, { subscription: "lease-1", last_seq: 0 });
    await tick();
    expect(lease.state).toEqual({ held: true });
    const event: EventEnvelope = { seq: null, ts: "t", event: { scope: "extension", id: "database", event: { done: true } } };
    socket.event("lease-1", event);
    expect(events).toEqual([event]);
    lease.close();
    await tick();
    expect(socket.last("events.unsubscribe").params).toEqual({ subscription: "lease-1" });
    expect(states).toEqual([{ held: true }]);
    client.stop();
  });

  test("a refused lease (the package is starting) is asked again until the daemon accepts it", async () => {
    const { client, connect } = harness();
    client.start();
    const socket = await connect();
    const lease = client.watchExtension("office", () => {}, () => {});
    await tick();
    socket.fail(socket.last("extensions.watch").id, 503, "The extension is changing versions");
    await tick();
    expect(lease.state).toEqual({ held: false, error: "The extension is changing versions" });
    lease.retry();
    await tick();
    const again = socket.sent.filter((message) => message.method === "extensions.watch");
    expect(again).toHaveLength(2);
    socket.reply(again[1]!.id, { subscription: "lease-2", last_seq: 0 });
    await tick();
    expect(lease.state).toEqual({ held: true });
    lease.close();
    client.stop();
  });

  test("a lease lost with the connection says so, then is taken again after the reconnect", async () => {
    const { client, connect } = harness();
    client.start();
    const first = await connect();
    const states: unknown[] = [];
    const lease = client.watchExtension("database", () => {}, (state) => states.push(state));
    await tick();
    first.reply(first.last("extensions.watch").id, { subscription: "lease-1", last_seq: 0 });
    await tick();
    first.drop();
    expect(lease.state).toEqual({ held: false, error: null });
    await wait(20);
    const second = await connect();
    const watch = second.last("extensions.watch");
    second.reply(watch.id, { subscription: "lease-3", last_seq: 0 });
    await tick();
    expect(states).toEqual([{ held: true }, { held: false, error: null }, { held: true }]);
    expect(second.sent.some((message) => message.method === "events.subscribe")).toBe(false);
    lease.close();
    client.stop();
  });

  test("a lease closed before the daemon answered is given back at once", async () => {
    const { client, connect } = harness();
    client.start();
    const socket = await connect();
    const lease = client.watchExtension("database", () => {}, () => {});
    await tick();
    const watch = socket.last("extensions.watch");
    lease.close();
    socket.reply(watch.id, { subscription: "late", last_seq: 0 });
    await tick();
    expect(socket.last("events.unsubscribe").params).toEqual({ subscription: "late" });
    expect(lease.state).toEqual({ held: false, error: null });
    client.stop();
  });
});
