// Logs Codex back in to its ChatGPT account, as `codex login` does: an app-server starts the
// OAuth flow (`account/login/start`) and gives the page to open; the browser comes back to its
// local callback, which stores the credentials in Codex's home. Chauffeur never sees the token.
// Headless (the browser on another machine, a worker signed in from the Mac), it is the device
// code flow instead, as `codex login --device-auth`: the user types the code on OpenAI's page and
// the app-server, which polls for it, stores the credentials itself.

import { homedir } from "node:os";
import { errorMessage, type Logger } from "../../log.ts";
import type { AgentLoginResult } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";
import { ErrorCode } from "../../protocol.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import type { CodexSessionDeps } from "./session.ts";

/** How long the login may take in the browser before the app-server is ended. */
export const LOGIN_TIMEOUT_MS = 10 * 60_000;
const START_TIMEOUT_MS = 30_000;

/** The login running, if any: a new one replaces it, unreported (its successor is). */
let current: { cancel: (reported: boolean) => void } | undefined;

export async function codexLogin(
  deps: CodexSessionDeps,
  log: Logger,
  ended: (success: boolean, error: string | null) => void,
  headless = false,
  timeoutMs = LOGIN_TIMEOUT_MS,
): Promise<AgentLoginResult> {
  current?.cancel(false);
  let loginId: string | undefined;
  /** A completion that came before `account/login/start` answered. */
  let early: { loginId: string | null; success: boolean; error: string | null } | undefined;
  let finished = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const server = deps.spawn({
    executable: deps.resolveExecutable(deps.baseEnv),
    args: [],
    cwd: homedir(),
    env: deps.baseEnv,
    handlers: {
      notification: (message) => {
        if (message.method !== "account/login/completed") return;
        const done = message.params;
        if (loginId === undefined) early = done;
        else if (done.loginId === null || done.loginId === loginId) finish(done.success, done.error);
      },
      request: async () => {
        throw new RpcError(ErrorCode.METHOD_NOT_FOUND, "not supported while logging Codex in");
      },
    },
    log,
  });
  const finish = (success: boolean, error: string | null, reported = true) => {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    void server.close();
    if (current === self) current = undefined;
    if (reported) ended(success, error);
  };
  const self = {
    cancel: (reported: boolean) => {
      if (loginId) void server.request("account/login/cancel", { loginId }).catch(() => {});
      finish(false, "the login was cancelled", reported);
    },
  };
  current = self;
  let result: AgentLoginResult;
  try {
    const started = await Promise.race([
      (async () => {
        await server.request("initialize", {
          clientInfo: { name: "chauffeur", title: "Chauffeur", version: SIDECAR_VERSION },
          capabilities: { experimentalApi: false, requestAttestation: false, optOutNotificationMethods: [] },
        });
        server.notify("initialized");
        return server.request("account/login/start", { type: headless ? "chatgptDeviceCode" : "chatgpt" });
      })(),
      server.exited.then((reason) => Promise.reject(new Error(`codex app-server ${reason}`))),
      new Promise<never>((_, reject) => setTimeout(() => reject(new Error("no answer")), START_TIMEOUT_MS)),
    ]);
    if (started.type === "chatgpt" && !headless) {
      loginId = started.loginId;
      result = { url: started.authUrl };
    } else if (started.type === "chatgptDeviceCode" && headless) {
      loginId = started.loginId;
      result = { url: started.verificationUrl, user_code: started.userCode };
    } else {
      throw new Error(`unexpected login type ${started.type}`);
    }
  } catch (error) {
    finished = true;
    void server.close();
    if (current === self) current = undefined;
    throw new Error(`Codex could not start its login: ${errorMessage(error)}`);
  }
  timer = setTimeout(() => {
    void server.request("account/login/cancel", { loginId: loginId! }).catch(() => {});
    // OpenAI refuses a device code on its page only when the account has not turned that sign-in
    // on: the app-server just never hears back.
    finish(
      false,
      headless
        ? "the login was not completed in time; if OpenAI refused the code, turn device code sign-in on in ChatGPT → Settings → Security, then sign in again"
        : "the login was not completed in time",
    );
  }, timeoutMs);
  void server.exited.then((reason) => finish(false, `codex app-server ${reason}`));
  if (early && (early.loginId === null || early.loginId === loginId)) finish(early.success, early.error);
  return result;
}

/** Ends the login under way, reported as not completed; nothing when none runs. */
export function codexLoginCancel(): void {
  current?.cancel(true);
}
