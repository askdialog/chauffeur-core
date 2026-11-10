// JSON-RPC 2.0 over newline-delimited JSON, symmetric: both ends send requests, notifications and
// responses. The sidecar reads the daemon on stdin and writes to it on stdout.

import { ErrorCode, type JsonValue, type RpcErrorObject } from "../protocol.ts";
import { errorMessage, silentLogger, type Logger } from "../log.ts";
import { LineSplitter } from "../util/lines.ts";

export type RpcId = number | string;

/** An error that crosses the wire as `{code, message, data}`. */
export class RpcError extends Error {
  readonly code: number;
  readonly data: JsonValue | undefined;

  constructor(code: number, message: string, data?: JsonValue) {
    super(message);
    this.name = "RpcError";
    this.code = code;
    this.data = data;
  }

  toObject(): RpcErrorObject {
    return this.data === undefined
      ? { code: this.code, message: this.message }
      : { code: this.code, message: this.message, data: this.data };
  }

  static from(error: unknown): RpcError {
    if (error instanceof RpcError) return error;
    return new RpcError(ErrorCode.INTERNAL, errorMessage(error));
  }
}

export interface PeerHandlers {
  /** Answers a request; the returned value becomes `result` (`{}` when undefined). */
  onRequest(method: string, params: unknown, id: RpcId): unknown;
  /** Receives a notification. Errors are logged, never answered. */
  onNotification?(method: string, params: unknown): void;
}

interface PendingRequest {
  method: string;
  resolve: (result: unknown) => void;
  reject: (error: RpcError) => void;
}

/**
 * One end of a JSON-RPC connection. `write` receives complete lines (newline included); `run`
 * consumes the input stream until it ends. Requests are handled concurrently: each handler is
 * invoked synchronously, in arrival order, and its response is written whenever it settles.
 */
export class JsonRpcPeer {
  private readonly write: (line: string) => void;
  private readonly handlers: PeerHandlers;
  private readonly log: Logger;
  private readonly pending = new Map<RpcId, PendingRequest>();
  private nextId = 1;
  private closed = false;

  constructor(options: { write: (line: string) => void; handlers: PeerHandlers; log?: Logger }) {
    this.write = options.write;
    this.handlers = options.handlers;
    this.log = options.log ?? silentLogger;
  }

  get isClosed(): boolean {
    return this.closed;
  }

  /** Reads newline-delimited JSON until the input ends, then closes the peer. */
  async run(input: AsyncIterable<Uint8Array | string>): Promise<void> {
    const lines = new LineSplitter();
    const receive = (line: string) => this.receive(line);
    try {
      for await (const chunk of input) lines.push(chunk, receive);
      const rest = lines.end();
      if (rest.trim() !== "") this.receive(rest);
    } finally {
      this.close("the input stream ended");
    }
  }

  /** Handles one line of input. */
  receive(line: string): void {
    const text = line.trim();
    if (text === "") return;
    let message: unknown;
    try {
      message = JSON.parse(text);
    } catch (error) {
      this.log.warn("unparsable line", errorMessage(error));
      this.send({ jsonrpc: "2.0", id: null, error: { code: ErrorCode.PARSE_ERROR, message: "Parse error" } });
      return;
    }
    if (!isObject(message)) {
      this.send({ jsonrpc: "2.0", id: null, error: { code: ErrorCode.INVALID_REQUEST, message: "Invalid request" } });
      return;
    }
    const id = message.id;
    const hasId = id !== undefined && id !== null;
    if (hasId && !isRpcId(id)) {
      this.send({ jsonrpc: "2.0", id: null, error: { code: ErrorCode.INVALID_REQUEST, message: "Invalid id" } });
      return;
    }
    if (typeof message.method === "string") {
      if (hasId) this.handleRequest(id as RpcId, message.method, message.params);
      else this.handleNotification(message.method, message.params);
      return;
    }
    if (hasId && ("result" in message || "error" in message)) {
      this.handleResponse(id as RpcId, message);
      return;
    }
    this.send({
      jsonrpc: "2.0",
      id: hasId ? id : null,
      error: { code: ErrorCode.INVALID_REQUEST, message: "Invalid request" },
    });
  }

  /** Sends a request and resolves with its result, or rejects with an RpcError. */
  request<R = unknown>(method: string, params: unknown): Promise<R> {
    if (this.closed) return Promise.reject(new RpcError(ErrorCode.UNAVAILABLE, "connection closed"));
    const id = this.nextId++;
    return new Promise<R>((resolve, reject) => {
      this.pending.set(id, { method, resolve: resolve as (result: unknown) => void, reject });
      this.send({ jsonrpc: "2.0", id, method, params });
    });
  }

  notify(method: string, params: unknown): void {
    if (this.closed) return;
    this.send({ jsonrpc: "2.0", method, params });
  }

  /** Rejects every request still waiting for a response. Further sends are dropped. */
  close(reason: string): void {
    if (this.closed) return;
    this.closed = true;
    for (const [id, request] of this.pending) {
      this.pending.delete(id);
      request.reject(new RpcError(ErrorCode.UNAVAILABLE, `${request.method}: ${reason}`));
    }
  }

  private handleRequest(id: RpcId, method: string, params: unknown): void {
    let outcome: unknown;
    try {
      outcome = this.handlers.onRequest(method, params, id);
    } catch (error) {
      this.respondError(id, method, error);
      return;
    }
    Promise.resolve(outcome).then(
      (result) => this.send({ jsonrpc: "2.0", id, result: result ?? {} }),
      (error: unknown) => this.respondError(id, method, error),
    );
  }

  private respondError(id: RpcId, method: string, error: unknown): void {
    const rpcError = RpcError.from(error);
    if (rpcError.code === ErrorCode.INTERNAL) this.log.error(`${method} failed`, error);
    else this.log.debug(`${method} refused: ${rpcError.message}`);
    this.send({ jsonrpc: "2.0", id, error: rpcError.toObject() });
  }

  private handleNotification(method: string, params: unknown): void {
    try {
      this.handlers.onNotification?.(method, params);
    } catch (error) {
      this.log.error(`notification ${method} failed`, error);
    }
  }

  private handleResponse(id: RpcId, message: Record<string, unknown>): void {
    const request = this.pending.get(id);
    if (!request) {
      this.log.warn(`response to unknown request ${JSON.stringify(id)}`);
      return;
    }
    this.pending.delete(id);
    const error = message.error;
    if (error !== undefined && error !== null) {
      const code = isObject(error) && typeof error.code === "number" ? error.code : ErrorCode.INTERNAL;
      const text = isObject(error) && typeof error.message === "string" ? error.message : "unknown error";
      const data = isObject(error) ? (error.data as JsonValue | undefined) : undefined;
      request.reject(new RpcError(code, text, data ?? undefined));
    } else {
      request.resolve(message.result);
    }
  }

  private send(message: Record<string, unknown>): void {
    if (this.closed && !("id" in message && ("result" in message || "error" in message))) return;
    let line: string;
    try {
      line = JSON.stringify(message);
    } catch (error) {
      this.log.error("cannot serialize an outgoing message", error);
      return;
    }
    try {
      this.write(`${line}\n`);
    } catch (error) {
      this.log.error("cannot write an outgoing message", error);
    }
  }
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isRpcId(value: unknown): value is RpcId {
  return typeof value === "string" || (typeof value === "number" && Number.isFinite(value));
}
