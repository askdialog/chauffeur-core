// Wires the JSON-RPC peer, the dispatcher and the session manager. main.ts plugs it into stdio;
// tests plug it into in-memory streams.

import type { Agent } from "./agents/agent.ts";
import { createDispatcher } from "./dispatch.ts";
import type { Logger } from "./log.ts";
import { ErrorCode, type DaemonHookMethods, type SidecarNotifications } from "./protocol.ts";
import { JsonRpcPeer, RpcError } from "./rpc/peer.ts";
import { SessionManager } from "./sessions.ts";

export interface Sidecar {
  peer: JsonRpcPeer;
  sessions: SessionManager;
  /** Serves until the input ends, then stops every session. */
  serve(input: AsyncIterable<Uint8Array | string>): Promise<void>;
}

export function createSidecar(options: { agents: Agent[]; write: (line: string) => void; log: Logger }): Sidecar {
  const { log } = options;
  // The manager reaches the daemon through the peer, built right after it.
  let peer: JsonRpcPeer | undefined;
  const connection = (): JsonRpcPeer => {
    if (!peer) throw new RpcError(ErrorCode.UNAVAILABLE, "not connected");
    return peer;
  };
  const notify = <M extends keyof SidecarNotifications>(method: M, params: SidecarNotifications[M]) =>
    connection().notify(method, params);
  const request = <M extends keyof DaemonHookMethods>(method: M, params: DaemonHookMethods[M][0]) =>
    connection().request<DaemonHookMethods[M][1]>(method, params);

  const sessions = new SessionManager(
    options.agents,
    {
      emit: (chatId, event) => notify("session.event", { chat_id: chatId, event }),
      createWorktree: async (chatId, name) => {
        const result = await request("hook.worktree_create", { chat_id: chatId, name });
        if (typeof result?.path !== "string" || result.path === "") {
          throw new RpcError(ErrorCode.INTERNAL, "hook.worktree_create returned no path");
        }
        return result.path;
      },
      removeWorktree: async (chatId, path) => {
        await request("hook.worktree_remove", { chat_id: chatId, path });
      },
      listRepos: (chatId) => request("tool.list_repos", { chat_id: chatId }),
      addRepo: (chatId, repo) => request("tool.add_repo", { chat_id: chatId, repo }),
      delegate: (params) => request("tool.delegate", params),
      delegateWait: (chatId, child) => request("tool.delegate_wait", { chat_id: chatId, child }),
      delegateStop: async (chatId, child) => {
        await request("tool.delegate_stop", { chat_id: chatId, child });
      },
      delegateDetach: async (chatId, child) => {
        await request("tool.delegate_detach", { chat_id: chatId, child });
      },
      loginEnded: (params) => notify("agent.login_ended", params),
    },
    log,
  );
  const dispatch = createDispatcher(sessions);
  peer = new JsonRpcPeer({
    write: options.write,
    log: log.child("rpc"),
    handlers: {
      onRequest: dispatch,
      onNotification: (method) => log.warn(`ignoring notification ${method}`),
    },
  });

  return {
    peer,
    sessions,
    async serve(input) {
      await connection().run(input);
      log.info(`input closed: stopping ${sessions.size} session(s)`);
      await sessions.stopAll();
    },
  };
}
