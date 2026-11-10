import type { AccountInfo } from "@anthropic-ai/claude-agent-sdk";
import type { NativeAccountInfo } from "../../protocol.ts";
import type { ClaudeSessionDeps } from "./session.ts";
import { idleClaude, signedOut } from "./usage.ts";

function text(value: unknown): string | null {
  return typeof value === "string" && value.trim().length > 0 && value.length <= 256 && !/[\u0000-\u001f\u007f-\u009f]/.test(value) ? value : null;
}
/** Unknown sources remain unsupported; never guess OAuth from email alone or prefer API fallback. */
export function accountMetadata(value: AccountInfo): NativeAccountInfo {
  const none = (v: string | undefined) => !v || v === "none";
  const external = value.apiProvider !== undefined && value.apiProvider !== "firstParty";
  // The installed CLI omits tokenSource for a stored subscription and returns subscriptionType.
  // Environment bearer/OAuth origins and unknown plans stay unsupported even if an email exists.
  const subscription = value.tokenSource === "claude.ai" || (none(value.tokenSource) && ["pro", "max", "team", "enterprise"].includes(value.subscriptionType ?? ""));
  const mode = external ? "external" : !none(value.apiKeySource) ? "api_key" : signedOut(value) ? "signed_out"
    : subscription ? "subscription" : "unknown";
  return { mode, email: text(value.email), organization: text(value.organization) };
}
export async function readNativeAccount(deps: ClaudeSessionDeps): Promise<NativeAccountInfo> {
  let close: (() => void) | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const idle = idleClaude(deps);
    close = idle.close;
    const info = await Promise.race([
      idle.query.accountInfo(),
      new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error()), 20_000); }),
    ]);
    return accountMetadata(info);
  } catch {
    throw new Error("Could not inspect the native Claude Code account; check its local login");
  } finally {
    clearTimeout(timer);
    close?.();
  }
}
