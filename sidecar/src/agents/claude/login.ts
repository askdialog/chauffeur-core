// Logs Claude Code back in to its claude.ai account, as `/login` does: an idle CLI starts the
// OAuth flow and gives the page to open; the browser comes back to the CLI's own local callback,
// which stores the credentials where Claude Code keeps them. Chauffeur never sees the token.
// Headless (the browser on another machine, a worker signed in from the Mac), the manual page
// instead shows a code the user pastes back (`claudeLoginCode`); the CLI alone can exchange it,
// holding the flow's PKCE secret.

import { errorMessage } from "../../log.ts";
import { ErrorCode, type AgentLoginResult } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";
import type { ClaudeSessionDeps } from "./session.ts";
import { idleClaude } from "./usage.ts";

/** How long the login may take in the browser before the CLI is ended. */
export const LOGIN_TIMEOUT_MS = 10 * 60_000;
const START_TIMEOUT_MS = 30_000;

type Urls = { manualUrl?: string; automaticUrl?: string };
type LoginQuery = {
  claudeAuthenticate?: (loginWithClaudeAi: boolean) => Promise<Urls>;
  claudeOAuthCallback?: (authorizationCode: string, state?: string) => Promise<unknown>;
  claudeOAuthWaitForCompletion?: () => Promise<unknown>;
};

type Login = {
  /** Ends it; a login replaced by a newer one is not reported, its successor is. */
  cancel: (reported: boolean) => void;
  /** Gives a headless login the code its page showed; absent otherwise. */
  submit?: (code: string) => Promise<void>;
};

/** The login running, if any: a new one replaces it. */
let current: Login | undefined;

export async function claudeLogin(
  deps: ClaudeSessionDeps,
  ended: (success: boolean, error: string | null) => void,
  headless = false,
  timeoutMs = LOGIN_TIMEOUT_MS,
): Promise<AgentLoginResult> {
  current?.cancel(false);
  const { query, close } = idleClaude(deps);
  const login = query as unknown as LoginQuery;
  let finished = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const finish = (success: boolean, error: string | null, reported = true) => {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    close();
    if (current === self) current = undefined;
    if (reported) ended(success, error);
  };
  const self: Login = {
    cancel: (reported) => finish(false, "the login was cancelled", reported),
  };
  current = self;
  let urls: Urls;
  try {
    if (typeof login.claudeAuthenticate !== "function" || typeof login.claudeOAuthWaitForCompletion !== "function") {
      throw new Error("this Claude Agent SDK cannot log Claude Code in");
    }
    if (headless && typeof login.claudeOAuthCallback !== "function") {
      throw new Error("this Claude Agent SDK cannot take a login code");
    }
    urls = await Promise.race([
      login.claudeAuthenticate.call(query, true),
      new Promise<never>((_, reject) => setTimeout(() => reject(new Error("no answer")), START_TIMEOUT_MS)),
    ]);
  } catch (error) {
    finished = true;
    close();
    if (current === self) current = undefined;
    throw new Error(`Claude Code could not start its login: ${errorMessage(error)}`);
  }
  // The automatic page comes back to the CLI by itself; the manual one shows a code to paste.
  const url = headless ? urls.manualUrl : (urls.automaticUrl ?? urls.manualUrl);
  if (!url) {
    finish(false, "no login page");
    throw new Error("Claude Code gave no login page");
  }
  if (headless) {
    const callback = login.claudeOAuthCallback!;
    self.submit = async (code) => {
      // The page shows `<code>#<state>`, as the CLI's own prompt takes it.
      const [authorizationCode = "", state] = code.trim().split("#");
      try {
        await callback.call(query, authorizationCode, state);
      } catch (error) {
        throw new RpcError(ErrorCode.CONFLICT, `Claude Code refused the code: ${errorMessage(error)}; start the login again`);
      }
    };
  }
  timer = setTimeout(() => finish(false, "the login was not completed in time"), timeoutMs);
  login.claudeOAuthWaitForCompletion.call(query).then(
    () => finish(true, null),
    (error: unknown) => finish(false, errorMessage(error)),
  );
  return headless ? { url, asks_code: true } : { url };
}

/** Gives the headless login under way the code its page showed; resolves once it is signed in. */
export function claudeLoginCode(code: string): Promise<void> {
  if (!current?.submit) {
    throw new RpcError(ErrorCode.CONFLICT, "No Claude Code login waits for a code; start the login again");
  }
  return current.submit(code);
}

/** Ends the login under way, reported as not completed; nothing when none runs. */
export function claudeLoginCancel(): void {
  current?.cancel(true);
}
