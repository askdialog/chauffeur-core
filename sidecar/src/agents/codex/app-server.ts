// One `codex app-server` process (JSON-RPC over stdio, one JSON object per line), driven through
// the same JsonRpcPeer as the daemon link.

import { errorMessage, type Logger } from "../../log.ts";
import { JsonRpcPeer, type RpcId } from "../../rpc/peer.ts";
import type { CodexMethods, ServerNotification, ServerRequest } from "./protocol.ts";

export interface AppServerHandlers {
  notification(message: ServerNotification): void;
  /** Answers a server request (approvals…); the resolved value is the JSON-RPC result. */
  request(message: ServerRequest): Promise<unknown>;
}

export interface AppServer {
  /** Native process identity for daemon-owned cloud-session supervision. */
  readonly pid?: number;
  request<M extends keyof CodexMethods>(method: M, params: CodexMethods[M][0]): Promise<CodexMethods[M][1]>;
  notify(method: "initialized"): void;
  /** Resolves when the process is gone, with a short reason. */
  readonly exited: Promise<string>;
  /** Ends the process: closes its input, then terminates it if it lingers. */
  close(): Promise<void>;
}

export interface SpawnOptions {
  executable: string;
  /** Arguments after `app-server`. */
  args: string[];
  cwd: string;
  env: Record<string, string | undefined>;
  handlers: AppServerHandlers;
  log: Logger;
}

export type SpawnAppServer = (options: SpawnOptions) => AppServer;

const CLOSE_GRACE_MS = 3_000;

export const spawnAppServer: SpawnAppServer = ({ executable, args, cwd, env, handlers, log }) => {
  const proc = Bun.spawn([executable, "app-server", ...args], {
    cwd,
    env: definedEntries(env),
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
  });
  const peer = new JsonRpcPeer({
    log: log.child("app-server"),
    write: (line) => {
      proc.stdin.write(line);
      void proc.stdin.flush();
    },
    handlers: {
      onRequest: (method, params, id: RpcId) => handlers.request({ method, id, params } as ServerRequest),
      onNotification: (method, params) => handlers.notification({ method, params } as ServerNotification),
    },
  });
  const reading = peer.run(proc.stdout);
  void forwardStderr(proc.stderr, log);
  const exited = proc.exited.then(async (code) => {
    await reading.catch(() => {});
    peer.close("codex app-server exited");
    return proc.signalCode ? `killed by ${proc.signalCode}` : `exited with code ${code}`;
  });

  return {
    pid: proc.pid,
    request: (method, params) => peer.request(method, params),
    notify: (method) => peer.notify(method, undefined),
    exited,
    async close() {
      try {
        proc.stdin.end();
      } catch {
        // Already closed.
      }
      if (await settlesWithin(exited, CLOSE_GRACE_MS)) return;
      proc.kill("SIGTERM");
      if (await settlesWithin(exited, CLOSE_GRACE_MS)) return;
      proc.kill("SIGKILL");
      await exited;
    },
  };
};

async function forwardStderr(stream: ReadableStream<Uint8Array>, log: Logger): Promise<void> {
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    for await (const chunk of stream) {
      buffer += decoder.decode(chunk, { stream: true });
      let newline = buffer.indexOf("\n");
      while (newline >= 0) {
        const line = buffer.slice(0, newline).trimEnd();
        buffer = buffer.slice(newline + 1);
        if (line) log.debug(`codex: ${line}`);
        newline = buffer.indexOf("\n");
      }
    }
  } catch (error) {
    log.debug(`codex stderr closed: ${errorMessage(error)}`);
  }
}

function settlesWithin(promise: Promise<unknown>, ms: number): Promise<boolean> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return Promise.race([
    promise.then(() => true),
    new Promise<boolean>((resolve) => {
      timer = setTimeout(() => resolve(false), ms);
    }),
  ]).finally(() => clearTimeout(timer));
}

function definedEntries(env: Record<string, string | undefined>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) if (value !== undefined) result[key] = value;
  return result;
}
