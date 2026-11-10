// The Chauffeur wire protocol for TypeScript clients (desktop app, sidecar, mobile).
// Everything under ./generated comes from the Rust crate chauffeur-protocol (scripts/gen-protocol.sh),
// the method → params/result maps and the error codes included; this file adds the JSON-RPC envelopes.

export * from "./generated";

import type { ClientMethods, JsonValue, PeerMethods, RpcError } from "./generated";

/** What a process of the user's reaches on the daemon's Unix socket: `/ws`, and `/peer` for the
 * methods of another daemon (`connectDaemon` in relay/src/daemon.ts sends each where it belongs,
 * and says `hello` on each itself: `/peer`'s answers a `PeerHelloResult`, not a `HelloResult`). */
export type UnixSocketMethods = ClientMethods & Omit<PeerMethods, "hello">;

export type RpcRequest<M extends string = string, P = unknown> = {
  jsonrpc: "2.0";
  id: number | string;
  method: M;
  params: P;
};

export type RpcNotification<M extends string = string, P = unknown> = {
  jsonrpc: "2.0";
  method: M;
  params: P;
};

export type RpcResponse<R = JsonValue> = {
  jsonrpc: "2.0";
  id: number | string;
  result?: R;
  error?: RpcError;
};
