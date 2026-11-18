// One ACP agent process: JSON-RPC 2.0 over stdio, one JSON object per line, driven through the
// same JsonRpcPeer as the daemon link. The agent's stdout must be a pipe (Vibe refuses a file).

import { errorMessage, type Logger } from "../../log.ts";
import { JsonRpcPeer, type RpcId } from "../../rpc/peer.ts";
import { settlesWithin } from "../../util/settles-within.ts";

export interface AcpHandlers {
  /** A notification of the agent's (`session/update`…). */
  notification(method: string, params: unknown): void;
  /** Answers a request of the agent's (`session/request_permission`…); the value is the result. */
  request(method: string, params: unknown, id: RpcId): Promise<unknown>;
}

export interface AcpConnection {
  /** Native process identity for daemon-owned cloud-session supervision. */
  readonly pid?: number;
  request<T = unknown>(method: string, params: unknown): Promise<T>;
  notify(method: string, params: unknown): void;
  /** Resolves when the process is gone, with a short reason. */
  readonly exited: Promise<string>;
  /** Ends the process: closes its input, then terminates it if it lingers. */
  close(): Promise<void>;
}

export interface AcpSpawnOptions {
  executable: string;
  args: string[];
  cwd: string;
  env: Record<string, string | undefined>;
  handlers: AcpHandlers;
  log: Logger;
  /** How the agent is named in logs, `vibe-acp` say. */
  name: string;
}

export type SpawnAcp = (options: AcpSpawnOptions) => AcpConnection;

const CLOSE_GRACE_MS = 3_000;

export const spawnAcp: SpawnAcp = ({ executable, args, cwd, env, handlers, log, name }) => {
  const proc = Bun.spawn([executable, ...args], {
    cwd,
    env: definedEntries(env),
    stdin: "pipe",
    stdout: "pipe",
    stderr: "pipe",
  });
  const peer = new JsonRpcPeer({
    log: log.child(name),
    write: (line) => {
      proc.stdin.write(line);
      void proc.stdin.flush();
    },
    handlers: {
      onRequest: (method, params, id) => handlers.request(method, params, id),
      onNotification: (method, params) => handlers.notification(method, params),
    },
  });
  const reading = peer.run(proc.stdout);
  void forwardStderr(proc.stderr, log, name);
  const exited = proc.exited.then(async (code) => {
    await reading.catch(() => {});
    peer.close(`${name} exited`);
    return proc.signalCode ? `killed by ${proc.signalCode}` : `exited with code ${code}`;
  });

  return {
    pid: proc.pid,
    request: <T>(method: string, params: unknown) => peer.request(method, params) as Promise<T>,
    notify: (method, params) => peer.notify(method, params),
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

async function forwardStderr(stream: ReadableStream<Uint8Array>, log: Logger, name: string): Promise<void> {
  const decoder = new TextDecoder();
  let buffer = "";
  try {
    for await (const chunk of stream) {
      buffer += decoder.decode(chunk, { stream: true });
      let newline = buffer.indexOf("\n");
      while (newline >= 0) {
        const line = buffer.slice(0, newline).trimEnd();
        buffer = buffer.slice(newline + 1);
        if (line) log.debug(`${name}: ${line}`);
        newline = buffer.indexOf("\n");
      }
    }
  } catch (error) {
    log.debug(`${name} stderr closed: ${errorMessage(error)}`);
  }
}

function definedEntries(env: Record<string, string | undefined>): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) if (value !== undefined) result[key] = value;
  return result;
}
