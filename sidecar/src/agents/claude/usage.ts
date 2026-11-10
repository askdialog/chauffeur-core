// What is left of the claude.ai plan's limits (the 5-hour and weekly windows), as Claude Code's
// /usage reads them: an idle CLI, asked through the SDK's usage request, then ended. Claude Code
// keeps its login fresh itself; an API key has no plan limits (no windows). The same CLI tells
// whether it is logged in at all (`accountInfo`).

import { homedir } from "node:os";
import type { AccountInfo, Query, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import { errorMessage } from "../../log.ts";
import type { AgentUsageResult, ExtraUsage, UsageWindow } from "../../protocol.ts";
import { AsyncQueue } from "../../util/async-queue.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import type { ClaudeSessionDeps } from "./session.ts";

const USAGE_TIMEOUT_MS = 20_000;
const HOUR = 3600;
const WEEK = 7 * 24 * HOUR;

type Window = { utilization?: number | null; resets_at?: string | null } | null | undefined;
/** The part of the SDK's usage answer read here. */
export type ClaudeUsage = {
  subscription_type: string | null;
  rate_limits_available: boolean;
  rate_limits: {
    five_hour?: Window;
    seven_day?: Window;
    seven_day_opus?: Window;
    seven_day_sonnet?: Window;
    model_scoped?: Array<{ display_name: string; utilization: number | null; resets_at: string | null }>;
    /** Spend beyond the plan's limits this month; amounts in minor units of `currency` (cents for USD). */
    extra_usage?: {
      is_enabled: boolean;
      monthly_limit: number | null;
      used_credits: number | null;
      utilization: number | null;
      currency?: string | null;
    } | null;
  } | null;
};

/** The SDK names its usage request as experimental: reached by name, so a rename only hides it. */
const USAGE_METHOD = "usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET";

export const CLAUDE_SIGNED_OUT = "Claude Code is not logged in";

/** An idle CLI, ended with `close`: it answers control requests without a turn. */
export function idleClaude(deps: ClaudeSessionDeps): { query: Query & Record<string, unknown>; close: () => void } {
  const abortController = new AbortController();
  const executable = deps.resolveExecutable();
  // No message: the CLI starts, answers requests and is ended.
  const input = new AsyncQueue<SDKUserMessage>();
  const query = deps.query({
    prompt: input,
    options: {
      tools: [],
      settingSources: [],
      persistSession: false,
      cwd: homedir(),
      env: { CLAUDE_AGENT_SDK_CLIENT_APP: `chauffeur-sidecar/${SIDECAR_VERSION}`, ...deps.baseEnv },
      abortController,
      ...(executable ? { pathToClaudeCodeExecutable: executable } : {}),
    },
  }) as Query & Record<string, unknown>;
  return {
    query,
    close: () => {
      input.end();
      abortController.abort();
    },
  };
}

export async function claudeUsage(deps: ClaudeSessionDeps): Promise<AgentUsageResult> {
  const { query, close } = idleClaude(deps);
  const ask = query[USAGE_METHOD];
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(`no answer after ${USAGE_TIMEOUT_MS / 1000}s`)), USAGE_TIMEOUT_MS);
  });
  try {
    const account = await Promise.race([query.accountInfo(), timeout]);
    if (signedOut(account)) return { plan: null, windows: [], extra_usage: null, signed_out: CLAUDE_SIGNED_OUT };
    if (typeof ask !== "function") throw new Error("this Claude Agent SDK has no usage request");
    const answer = (ask as (opts: { skipBehaviors: boolean }) => Promise<ClaudeUsage>).call(query, { skipBehaviors: true });
    const usage = await Promise.race([answer, timeout]);
    return claudeUsageResult(usage);
  } catch (error) {
    const message = errorMessage(error);
    // Logged in once, but its token is refused now.
    if (refusedCredentials(message)) return { plan: null, windows: [], extra_usage: null, signed_out: message };
    throw new Error(`Claude Code could not read its usage: ${message}`);
  } finally {
    clearTimeout(timer);
    close();
  }
}

/**
 * Logged in to nothing: no claude.ai account, no API key, on Anthropic's own API (a cloud
 * provider or a gateway authenticates outside Claude Code).
 */
export function signedOut(account: AccountInfo): boolean {
  if (account.apiProvider !== undefined && account.apiProvider !== "firstParty") return false;
  const none = (value: string | undefined) => value === undefined || value === "" || value === "none";
  return none(account.email) && none(account.tokenSource) && none(account.apiKeySource);
}

/** An error that says the credentials were refused rather than the request failing. */
export function refusedCredentials(message: string): boolean {
  return /\b401\b|unauthori[sz]ed|authentication_failed|invalid[ _-]?(api[ _-]?key|token|grant)|token (has )?(expired|revoked)|not logged in|please (run )?\/?login|log ?in again/i.test(
    message,
  );
}

export function claudeUsageResult(usage: ClaudeUsage): AgentUsageResult {
  const limits = usage.rate_limits_available ? usage.rate_limits : null;
  const windows: UsageWindow[] = [];
  const add = (label: string, window: Window, secs: number) => {
    if (window?.utilization == null) return;
    windows.push({ label, used_percent: window.utilization, resets_at: window.resets_at ?? null, window_secs: secs });
  };
  add("5h", limits?.five_hour, 5 * HOUR);
  add("7d", limits?.seven_day, WEEK);
  add("7d Opus", limits?.seven_day_opus, WEEK);
  add("7d Sonnet", limits?.seven_day_sonnet, WEEK);
  for (const scoped of limits?.model_scoped ?? []) add(`7d ${scoped.display_name}`, scoped, WEEK);
  return { plan: usage.subscription_type, windows, extra_usage: extraUsage(limits?.extra_usage), signed_out: null };
}

/** Kept when the plan has extra usage turned on, or spent some this month. */
function extraUsage(extra: NonNullable<ClaudeUsage["rate_limits"]>["extra_usage"]): ExtraUsage | null {
  if (!extra || (!extra.is_enabled && !extra.used_credits)) return null;
  return {
    enabled: extra.is_enabled,
    used: extra.used_credits ?? null,
    limit: extra.monthly_limit ?? null,
    currency: extra.currency ?? null,
  };
}
