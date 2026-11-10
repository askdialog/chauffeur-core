import { expect, test } from "bun:test";
import { estimateTokenCost } from "../src/agents/codex/pricing.ts";
import type { TokenUsageBreakdown } from "../src/agents/codex/protocol.ts";

const usage: TokenUsageBreakdown = {
  inputTokens: 100_000, cachedInputTokens: 60_000, cacheWriteInputTokens: 10_000,
  outputTokens: 20_000, reasoningOutputTokens: 15_000, totalTokens: 120_000,
};

test("cached reads and writes have their own rates, reasoning is not charged twice", () => {
  expect(estimateTokenCost("gpt-6.1-sol", usage, false)).toBeCloseTo(0.291);
  expect(estimateTokenCost("gpt-6.1-sol-2026-10-01", usage, true)).toBeCloseTo(0.582);
});

test("long context rates depend on each request, not the thread's accumulated tokens", () => {
  expect(estimateTokenCost("gpt-6.1-sol", { ...usage, inputTokens: 272_000 }, false)).toBeCloseTo(0.635);
  expect(estimateTokenCost("gpt-6.1-sol", { ...usage, inputTokens: 272_001 }, false)).toBeCloseTo(1.170004);
  expect(estimateTokenCost("gpt-6.1-sol", { ...usage, inputTokens: 272_001 }, true)).toBeCloseTo(2.340008);
});

test("unknown, local and invalid usage have no invented price", () => {
  for (const model of [null, "unknown", "constructor", "ollama/gpt-6.1-sol"]) expect(estimateTokenCost(model, usage, false)).toBeNull();
  expect(estimateTokenCost("gpt-6-luna", { ...usage, outputTokens: NaN }, false)).toBeNull();
});
