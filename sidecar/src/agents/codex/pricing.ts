// USD per million tokens, checked against https://developers.openai.com/api/docs/pricing
// on 2026-10-29. Unknown and local models have no estimate; never guess another model's rate.
import type { TokenUsageBreakdown } from "./protocol.ts";

const RATES: Record<string, { input: number; cached: number; output: number; longContext: boolean }> = {
  "gpt-6-astra": { input: 10, cached: 1, output: 50, longContext: true },
  "gpt-6.1-sol": { input: 2, cached: 0.1, output: 10, longContext: true },
  "gpt-6-sol": { input: 2, cached: 0.2, output: 10, longContext: true },
  "gpt-6.1-terra": { input: 1, cached: 0.05, output: 5, longContext: true },
  "gpt-6-luna": { input: 0.1, cached: 0.01, output: 0.5, longContext: true },
  "gpt-5.3-codex": { input: 1.75, cached: 0.175, output: 14, longContext: false },
};

/** API-equivalent token cost with default cache retention, excluding tools and subscription billing. Reasoning is in output. */
export function estimateTokenCost(model: string | null, usage: TokenUsageBreakdown, fast: boolean): number | null {
  const name = model?.replace(/-\d{4}-\d{2}-\d{2}$/, "");
  const rate = name && Object.hasOwn(RATES, name) ? RATES[name] : undefined;
  if (!rate) return null;
  const counts = [usage.inputTokens, usage.cachedInputTokens, usage.cacheWriteInputTokens, usage.outputTokens];
  if (counts.some((count) => !Number.isFinite(count) || count < 0)) return null;
  const input = Math.max(0, usage.inputTokens - usage.cachedInputTokens - usage.cacheWriteInputTokens);
  const long = rate.longContext && usage.inputTokens > 272_000;
  return (
    ((input * rate.input + usage.cachedInputTokens * rate.cached + usage.cacheWriteInputTokens * rate.input * 1.25) * (long ? 2 : 1) +
      usage.outputTokens * rate.output * (long ? 1.5 : 1)) *
    (fast ? 2 : 1) / 1_000_000
  );
}
