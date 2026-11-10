// The logins of workers' agents this app started (lib/workerLogin.ts), shared by the transfer
// views and Settings → Transfers. The Mac's daemon tells how each ended with a `login_ended`
// event naming the worker, and resumes the transfers that waited for it by itself.

import type { AgentKind } from "@chauffeur/protocol";
import { create } from "zustand";
import { openExternal } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { afterLoginEnded, loginKey, startedLogin, type WorkerLogin } from "@/lib/workerLogin";
import { client } from "./app";

export const useWorkerLogins = create<{ logins: Record<string, WorkerLogin> }>(() => ({ logins: {} }));

const put = (key: string, login: WorkerLogin) => useWorkerLogins.setState((s) => ({ logins: { ...s.logins, [key]: login } }));

let listening = false;

/** Opened at the first login: the ends of logins started before the app ran are not its own. */
function listen() {
  if (listening) return;
  listening = true;
  client.subscribe({ all: false, registry: true, workspaces: [], chats: [], ephemeral: false }, (envelope) => {
    if (envelope.event.scope !== "registry") return;
    const next = afterLoginEnded(useWorkerLogins.getState().logins, envelope.event.event);
    if (next) useWorkerLogins.setState({ logins: next });
  });
}

/** Starts the agent's login on the worker and opens its page in this Mac's browser. */
export async function signInOnWorker(worker: string, agent: AgentKind): Promise<void> {
  const key = loginKey(worker, agent);
  listen();
  put(key, { step: "starting" });
  try {
    const started = await client.call("worker.login", { name: worker, agent });
    put(key, startedLogin(started));
    await openExternal(started.url);
  } catch (error) {
    put(key, { step: "failed", error: errorMessage(error) });
  }
}

/** Passes the code Claude Code's page showed on to the worker; the login's end follows. */
export async function sendLoginCode(worker: string, agent: AgentKind, code: string): Promise<string | null> {
  const key = loginKey(worker, agent);
  const login = useWorkerLogins.getState().logins[key];
  if (login?.step !== "browser") return null;
  put(key, { ...login, sending: true });
  try {
    await client.call("worker.login_code", { name: worker, agent, code });
    return null;
  } catch (error) {
    const now = useWorkerLogins.getState().logins[key];
    if (now?.step === "browser") put(key, { ...now, sending: false });
    return errorMessage(error);
  }
}

export function cancelWorkerLogin(worker: string, agent: AgentKind) {
  const key = loginKey(worker, agent);
  useWorkerLogins.setState((s) => {
    const logins = { ...s.logins };
    delete logins[key];
    return { logins };
  });
  client.call("worker.login_cancel", { name: worker, agent }).catch(() => {});
}
