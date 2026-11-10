import { homedir } from "node:os";
import type { NativeAccountInfo } from "../../protocol.ts";
import type { Logger } from "../../log.ts";
import type { CodexSessionDeps } from "./session.ts";
import type { Account } from "./generated/v2/Account.ts";
import { probeCodex } from "./models.ts";

export function accountMetadata(account: Account | null): NativeAccountInfo {
  const email = account?.type === "chatgpt" && typeof account.email === "string" && account.email.trim().length > 0 && account.email.length <= 256 && !/[\u0000-\u001f\u007f-\u009f]/.test(account.email) ? account.email : null;
  return { mode: account === null ? "signed_out" : account.type === "chatgpt" ? "subscription" : account.type === "apiKey" ? "api_key" : "external", email, organization: null };
}
export async function readNativeAccount(deps: CodexSessionDeps, log: Logger): Promise<NativeAccountInfo> {
  try {
    return await probeCodex(deps, log, homedir(), "inspect its local account", async server => {
      const result = await server.request("account/read", { refreshToken: false });
      return accountMetadata(result.account);
    });
  } catch { throw new Error("Could not inspect the native Codex account; check its local login"); }
}
