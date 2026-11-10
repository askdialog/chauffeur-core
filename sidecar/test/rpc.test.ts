import { describe, expect, test } from "bun:test";
import { ErrorCode } from "../src/protocol.ts";
import { JsonRpcPeer, RpcError } from "../src/rpc/peer.ts";
import { AsyncQueue } from "../src/util/async-queue.ts";
import { flush } from "./support/flush.ts";

function peerWith(onRequest: (method: string, params: unknown) => unknown = () => ({})) {
  const lines: string[] = [];
  const notifications: Array<[string, unknown]> = [];
  const input = new AsyncQueue<Uint8Array | string>();
  const peer = new JsonRpcPeer({
    write: (line) => lines.push(line),
    handlers: { onRequest, onNotification: (method, params) => notifications.push([method, params]) },
  });
  const running = peer.run(input);
  const written = () => lines.map((line) => JSON.parse(line) as Record<string, unknown>);
  return { peer, input, lines, written, notifications, running };
}

describe("JSON-RPC framing", () => {
  test("answers a request with its result, one line per message", async () => {
    const { input, lines, written } = peerWith((method, params) => ({ method, params }));
    input.push('{"jsonrpc":"2.0","id":1,"method":"sidecar.ping","params":{"x":1}}\n');
    await flush();
    expect(lines).toHaveLength(1);
    expect(lines[0]?.endsWith("\n")).toBe(true);
    expect(lines[0]?.slice(0, -1)).not.toContain("\n");
    expect(written()[0]).toEqual({ jsonrpc: "2.0", id: 1, result: { method: "sidecar.ping", params: { x: 1 } } });
  });

  test("answers {} when the handler returns nothing, and keeps string ids", async () => {
    const { input, written } = peerWith(() => undefined);
    input.push('{"jsonrpc":"2.0","id":"a-1","method":"session.stop","params":{}}\n');
    await flush();
    expect(written()[0]).toEqual({ jsonrpc: "2.0", id: "a-1", result: {} });
  });

  test("turns handler failures into errors: RpcError keeps its code, anything else is internal", async () => {
    const { input, written } = peerWith((method) => {
      if (method === "sync") throw new RpcError(ErrorCode.NOT_FOUND, "no such chat");
      if (method === "async") return Promise.reject(new RpcError(ErrorCode.CONFLICT, "busy", { chat: "c" }));
      throw new Error("boom");
    });
    input.push('{"jsonrpc":"2.0","id":1,"method":"sync"}\n{"jsonrpc":"2.0","id":2,"method":"async"}\n');
    input.push('{"jsonrpc":"2.0","id":3,"method":"other"}\n');
    await flush();
    const byId = new Map(written().map((m) => [m.id, m]));
    expect(byId.get(1)?.error).toEqual({ code: 404, message: "no such chat" });
    expect(byId.get(2)?.error).toEqual({ code: 409, message: "busy", data: { chat: "c" } });
    expect(byId.get(3)?.error).toEqual({ code: -32603, message: "boom" });
  });

  test("reports unparsable lines and invalid messages without stopping", async () => {
    const { input, written } = peerWith(() => "ok");
    input.push("this is not json\n[1,2]\n{\"jsonrpc\":\"2.0\",\"id\":7}\n{\"jsonrpc\":\"2.0\",\"id\":8,\"method\":\"m\"}\n");
    await flush();
    const messages = written();
    expect(messages[0]).toEqual({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
    expect(messages[1]).toMatchObject({ id: null, error: { code: -32600 } });
    expect(messages[2]).toMatchObject({ id: 7, error: { code: -32600 } });
    expect(messages[3]).toEqual({ jsonrpc: "2.0", id: 8, result: "ok" });
  });

  test("delivers notifications without answering them", async () => {
    const { input, lines, notifications } = peerWith();
    input.push('{"jsonrpc":"2.0","method":"daemon.hello","params":{"v":1}}\n');
    await flush();
    expect(lines).toEqual([]);
    expect(notifications).toEqual([["daemon.hello", { v: 1 }]]);
  });

  test("reassembles messages split across chunks, including a split UTF-8 character", async () => {
    const { input, written } = peerWith((_, params) => params);
    const bytes = new TextEncoder().encode('{"jsonrpc":"2.0","id":1,"method":"m","params":{"text":"café ✓"}}\r\n');
    const cut = bytes.indexOf(0xc3) + 1; // inside "é"
    input.push(bytes.slice(0, 10));
    input.push(bytes.slice(10, cut));
    await flush();
    expect(written()).toEqual([]);
    input.push(bytes.slice(cut));
    await flush();
    expect(written()).toEqual([{ jsonrpc: "2.0", id: 1, result: { text: "café ✓" } }]);
  });

  test("reads a line of megabytes cut into thousands of chunks, with the next lines in its last chunk", async () => {
    const { input, written } = peerWith((_, params) => params);
    const big = "é".repeat(4 * 1024 * 1024);
    const bytes = new TextEncoder().encode(
      `{"jsonrpc":"2.0","id":1,"method":"m","params":{"text":"${big}"}}\n{"jsonrpc":"2.0","id":2,"method":"m","params":{"text":"a"}}\n{"jsonrpc":"2.0","id":3,"method":"m","params":{"text":"b"}}\n`,
    );
    const end = bytes.indexOf(10) - 3;
    // An odd size cuts two-byte characters across chunks.
    for (let start = 0; start < end; start += 4095) input.push(bytes.slice(start, Math.min(start + 4095, end)));
    await flush();
    expect(written()).toEqual([]);
    input.push(bytes.slice(end));
    await flush();
    const results = written().map((m) => (m.result as { text: string }).text);
    expect(results.map((text) => text.length)).toEqual([big.length, 1, 1]);
    expect(results[0] === big).toBe(true);
    expect(results.slice(1)).toEqual(["a", "b"]);
  });

  test("handles several messages in one chunk, blank lines, and a last line without newline", async () => {
    const { input, written, running } = peerWith((method) => method);
    input.push('{"jsonrpc":"2.0","id":1,"method":"a"}\n\n{"jsonrpc":"2.0","id":2,"method":"b"}\n{"jsonrpc":"2.0","id":3,"method":"c"}');
    await flush();
    expect(written().map((m) => m.id)).toEqual([1, 2]);
    input.end();
    await running;
    await flush();
    expect(written().map((m) => m.result)).toEqual(["a", "b", "c"]);
  });

  test("sends requests to the other side and resolves them with its responses", async () => {
    const { peer, input, written } = peerWith();
    const created = peer.request("hook.worktree_create", { chat_id: "c", name: "fix" });
    const removed = peer.request("hook.worktree_remove", { chat_id: "c", path: "/p" });
    const [first, second] = written();
    expect(first).toEqual({ jsonrpc: "2.0", id: 1, method: "hook.worktree_create", params: { chat_id: "c", name: "fix" } });
    expect(second).toMatchObject({ id: 2, method: "hook.worktree_remove" });
    // Answered out of order.
    input.push('{"jsonrpc":"2.0","id":2,"error":{"code":409,"message":"worktree in use"}}\n');
    input.push('{"jsonrpc":"2.0","id":1,"result":{"path":"/w/fix"}}\n');
    expect(await created).toEqual({ path: "/w/fix" });
    const error = await removed.catch((e: unknown) => e);
    expect(error).toBeInstanceOf(RpcError);
    expect(error).toMatchObject({ code: 409, message: "worktree in use" });
  });

  test("ignores responses to unknown ids", async () => {
    const { input, lines } = peerWith();
    input.push('{"jsonrpc":"2.0","id":99,"result":{}}\n');
    await flush();
    expect(lines).toEqual([]);
  });

  test("writes notifications", () => {
    const { peer, written } = peerWith();
    peer.notify("session.event", { chat_id: "c", event: { type: "turn_started" } });
    expect(written()).toEqual([
      { jsonrpc: "2.0", method: "session.event", params: { chat_id: "c", event: { type: "turn_started" } } },
    ]);
  });

  test("rejects pending and later requests once the input ends", async () => {
    const { peer, input, running } = peerWith();
    const pending = peer.request("hook.worktree_create", {});
    input.end();
    await running;
    await expect(pending).rejects.toMatchObject({ code: ErrorCode.UNAVAILABLE });
    await expect(peer.request("hook.worktree_remove", {})).rejects.toMatchObject({ code: ErrorCode.UNAVAILABLE });
    expect(peer.isClosed).toBe(true);
  });

  test("still answers requests that settle after the input ended", async () => {
    let release!: (value: string) => void;
    const { input, written, running } = peerWith(() => new Promise((resolve) => (release = resolve)));
    input.push('{"jsonrpc":"2.0","id":1,"method":"slow"}\n');
    await flush();
    input.end();
    await running;
    release("late");
    await flush();
    expect(written()).toEqual([{ jsonrpc: "2.0", id: 1, result: "late" }]);
  });
});
