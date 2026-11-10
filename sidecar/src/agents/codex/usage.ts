// What is left of the ChatGPT plan's Codex limits, as `codex app-server` reads them
// (`account/rateLimits/read`). Codex keeps its login fresh itself; `account/read` tells whether it
// is logged in at all.

import { homedir } from "node:os";
import { errorMessage, type Logger } from "../../log.ts";
import type { AgentUsageResult, UsageWindow } from "../../protocol.ts";
import type { GetAccountRateLimitsResponse } from "./generated/v2/GetAccountRateLimitsResponse.ts";
import type { RateLimitSnapshot } from "./generated/v2/RateLimitSnapshot.ts";
import type { RateLimitWindow } from "./generated/v2/RateLimitWindow.ts";
import { probeCodex } from "./models.ts";
import type { CodexSessionDeps } from "./session.ts";

export const CODEX_SIGNED_OUT = "Codex is not logged in";

export function readCodexUsage(deps: CodexSessionDeps, log: Logger): Promise<AgentUsageResult> {
  return probeCodex(deps, log, homedir(), "read its usage", async (server) => {
    const { account, requiresOpenaiAuth } = await server.request("account/read", { refreshToken: false });
    if (account === null && requiresOpenaiAuth) return { plan: null, windows: [], extra_usage: null, signed_out: CODEX_SIGNED_OUT };
    try {
      return codexUsageResult(await server.request("account/rateLimits/read", { excludeResetCreditDetails: false }));
    } catch (error) {
      // Its login was refused since `account/read` (a token revoked).
      const message = errorMessage(error);
      if (/authentication required|unauthori[sz]ed|\b401\b/i.test(message)) {
        return { plan: null, windows: [], extra_usage: null, signed_out: message };
      }
      throw error;
    }
  });
}

export function codexUsageResult(response: GetAccountRateLimitsResponse): AgentUsageResult {
  const windows: UsageWindow[] = [];
  const add = (snapshot: RateLimitSnapshot, name: string | null) => {
    for (const window of [snapshot.primary, snapshot.secondary]) {
      if (window) windows.push(usageWindow(window, name));
    }
  };
  add(response.rateLimits, null);
  for (const [id, snapshot] of Object.entries(response.rateLimitsByLimitId ?? {})) {
    if (!snapshot || id === (response.rateLimits.limitId ?? "codex")) continue;
    add(snapshot, snapshot.limitName ?? id);
  }
  // Its credits are a balance in credits, not money spent against a monthly limit: not shown.
  const resets = response.rateLimitResetCredits;
  return {
    plan: response.rateLimits.planType ?? null,
    windows,
    extra_usage: null,
    signed_out: null,
    ...(resets == null ? {} : {
      reset_credits: {
        available_count: Number(resets.availableCount),
        credits: resets.credits?.filter((credit) => credit.status === "available").map((credit) => ({
          title: credit.title ?? (credit.resetType === "codexRateLimits" ? "Full reset" : "Usage reset"),
          description: credit.description,
          expires_at: credit.expiresAt == null ? null : new Date(credit.expiresAt * 1000).toISOString(),
        })) ?? null,
      },
    }),
  };
}

function usageWindow(window: RateLimitWindow, name: string | null): UsageWindow {
  const mins = window.windowDurationMins;
  const span = mins == null ? "" : mins % 1440 === 0 ? `${mins / 1440}d` : `${Math.round(mins / 60)}h`;
  return {
    label: [span, name].filter(Boolean).join(" ") || "limit",
    used_percent: window.usedPercent,
    resets_at: window.resetsAt == null ? null : new Date(window.resetsAt * 1000).toISOString(),
    window_secs: mins == null ? null : mins * 60,
  };
}
