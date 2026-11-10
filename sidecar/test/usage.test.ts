import { describe, expect, test } from "bun:test";
import { claudeUsageResult, refusedCredentials, signedOut } from "../src/agents/claude/usage.ts";
import { codexUsageResult, readCodexUsage } from "../src/agents/codex/usage.ts";

import { silentLogger } from "../src/log.ts";
import { FakeAppServer, HOOK } from "./support/codex-fakes.ts";

describe("usage limits", () => {
  test("Claude's windows, the empty ones left out; an API key has none", () => {
    expect(
      claudeUsageResult({
        subscription_type: "max",
        rate_limits_available: true,
        rate_limits: {
          five_hour: { utilization: 36, resets_at: "2026-10-15T15:40:00Z" },
          seven_day: { utilization: 19, resets_at: "2026-10-21T11:00:00Z" },
          seven_day_opus: null,
          seven_day_sonnet: { utilization: null, resets_at: null },
          model_scoped: [{ display_name: "Fable", utilization: 0, resets_at: "2026-10-21T11:00:00Z" }],
        },
      }),
    ).toEqual({
      plan: "max",
      windows: [
        { label: "5h", used_percent: 36, resets_at: "2026-10-15T15:40:00Z", window_secs: 18000 },
        { label: "7d", used_percent: 19, resets_at: "2026-10-21T11:00:00Z", window_secs: 604800 },
        { label: "7d Fable", used_percent: 0, resets_at: "2026-10-21T11:00:00Z", window_secs: 604800 },
      ],
      extra_usage: null,
      signed_out: null,
    });
    expect(claudeUsageResult({ subscription_type: null, rate_limits_available: false, rate_limits: null })).toEqual({
      plan: null,
      windows: [],
      extra_usage: null,
      signed_out: null,
    });
  });

  test("Claude's extra usage, kept when turned on or spent, in minor units", () => {
    const extra = (extra_usage: unknown) =>
      claudeUsageResult({ subscription_type: "max", rate_limits_available: true, rate_limits: { extra_usage } as never }).extra_usage;
    expect(extra({ is_enabled: true, monthly_limit: 5000, used_credits: 1240, utilization: 24.8, currency: "USD" })).toEqual({
      enabled: true,
      used: 1240,
      limit: 5000,
      currency: "USD",
    });
    // Its limit reached: off, but what was spent still shows.
    expect(extra({ is_enabled: false, monthly_limit: 5000, used_credits: 5000, utilization: 100 })).toEqual({
      enabled: false,
      used: 5000,
      limit: 5000,
      currency: null,
    });
    expect(extra({ is_enabled: true, monthly_limit: null, used_credits: null, utilization: null, currency: "EUR" })).toEqual({
      enabled: true,
      used: null,
      limit: null,
      currency: "EUR",
    });
    expect(extra({ is_enabled: false, monthly_limit: null, used_credits: 0, utilization: null })).toBeNull();
    expect(extra(null)).toBeNull();
    expect(extra(undefined)).toBeNull();
  });

  test("Codex's windows, named after their length, other buckets after their name", () => {
    const snapshot = (limitId: string, limitName: string | null, primary: unknown, secondary: unknown) => ({
      limitId,
      limitName,
      normalModelSlug: null,
      primary,
      secondary,
      credits: null,
      individualLimit: null,
      spendControlReached: null,
      planType: "pro",
      rateLimitReachedType: null,
    });
    const main = snapshot("codex", null, { usedPercent: 12, windowDurationMins: 300, resetsAt: 1792080000 }, { usedPercent: 30, windowDurationMins: 10080, resetsAt: 1792500000 });
    expect(
      codexUsageResult({
        ordinaryUsageAllowed: true,
        rateLimits: main,
        rateLimitsByLimitId: { codex: main, spark: snapshot("spark", "Spark", null, { usedPercent: 5, windowDurationMins: 10080, resetsAt: null }) },
        rateLimitResetCredits: null,
        accountId: null,
        rateLimitUpsell: null,
      } as never),
    ).toEqual({
      plan: "pro",
      windows: [
        { label: "5h", used_percent: 12, resets_at: new Date(1792080000 * 1000).toISOString(), window_secs: 18000 },
        { label: "7d", used_percent: 30, resets_at: new Date(1792500000 * 1000).toISOString(), window_secs: 604800 },
        { label: "7d Spark", used_percent: 5, resets_at: null, window_secs: 604800 },
      ],
      extra_usage: null,
      signed_out: null,
    });
  });

  test("Claude is signed out when logged in to no account, no key, on Anthropic's own API", () => {
    // What a CLI without credentials answers.
    expect(signedOut({ tokenSource: "none", apiProvider: "firstParty" })).toBe(true);
    expect(signedOut({ email: "alice@example.com", subscriptionType: "Claude Max", apiProvider: "firstParty" })).toBe(false);
    expect(signedOut({ apiKeySource: "ANTHROPIC_API_KEY", apiProvider: "firstParty" })).toBe(false);
    // A cloud provider authenticates outside Claude Code.
    expect(signedOut({ apiProvider: "bedrock" })).toBe(false);
  });

  test("errors that mean refused credentials, not a failed request", () => {
    expect(refusedCredentials("API Error: 401 OAuth token has expired")).toBe(true);
    expect(refusedCredentials("Invalid API key · Please run /login")).toBe(true);
    expect(refusedCredentials("authentication_failed")).toBe(true);
    expect(refusedCredentials("no answer after 20s")).toBe(false);
    expect(refusedCredentials("API Error: 529 overloaded")).toBe(false);
  });
});


test("Codex keeps the reset count independent of capped details and preserves expiration", async () => {
  const response: Parameters<typeof codexUsageResult>[0] = {
    ordinaryUsageAllowed: true, accountId: null, rateLimitUpsell: null,
    rateLimitsByLimitId: null, rateLimitResetCredits: null,
    rateLimits: { limitId: "codex", limitName: null, normalModelSlug: null, primary: null,
      secondary: null, credits: null, individualLimit: null, spendControlReached: null,
      planType: "pro", rateLimitReachedType: null },
  };
  expect(codexUsageResult(response).reset_credits).toBeUndefined();
  response.rateLimitResetCredits = { availableCount: 0n, credits: [] };
  expect(codexUsageResult(response).reset_credits).toEqual({ available_count: 0, credits: [] });
  response.rateLimitResetCredits = { availableCount: 2n, credits: null };
  expect(codexUsageResult(response).reset_credits).toEqual({ available_count: 2, credits: null });
  const credit = { id: "reset", resetType: "codexRateLimits" as const, status: "available" as const,
    grantedAt: 1792080000, expiresAt: 1792500000, title: null, description: "Restores both limits" };
  response.rateLimitResetCredits.credits = [credit, { ...credit, status: "redeemed" }];
  expect(codexUsageResult(response).reset_credits).toEqual({ available_count: 2, credits: [{
    title: "Full reset", description: "Restores both limits", expires_at: new Date(1792500000 * 1000).toISOString(),
  }] });
  response.rateLimitResetCredits.credits = [{ ...credit, title: "Gift reset", expiresAt: null }];
  expect(codexUsageResult(response).reset_credits?.credits?.[0]).toEqual({
    title: "Gift reset", description: "Restores both limits", expires_at: null,
  });
  let server: FakeAppServer | undefined;
  const result = await readCodexUsage({
    spawn: (options) => {
      server = new FakeAppServer(options);
      server.rateLimits = response;
      return server;
    },
    resolveExecutable: () => "/usr/local/bin/codex",
    baseEnv: {},
    hookCommand: () => HOOK,
  }, silentLogger);
  expect(result.reset_credits?.available_count).toBe(2);
  expect(server?.sent("account/rateLimits/read")).toEqual([{ excludeResetCreditDetails: false }]);
  expect(server?.requests.map((request) => request.method)).toEqual(["initialize", "account/read", "account/rateLimits/read"]);
  expect(server?.closed).toBe(true);
});
