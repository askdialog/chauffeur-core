// Claude Code's `/context` reading (Query.getContextUsage) → the agent-agnostic ContextUsage.

import type { SDKControlGetContextUsageResponse } from "@anthropic-ai/claude-agent-sdk";
import type { ContextCategoryKind, ContextCompaction, ContextDetails, ContextUsage } from "../../protocol.ts";

const KINDS: readonly ContextCategoryKind[] = ["used", "free", "buffer", "deferred"];

/**
 * The reading as the app shows it. `jevThreshold`: Chauffeur compacts the chat through Jev from
 * that share of the context; otherwise Claude Code's autocompact does, where its buffer starts.
 * `details` adds the items behind the categories (MCP tools, memory files, skills, agents), kept
 * out of the per-turn event: hundreds of MCP tools would bloat the log.
 */
export function contextUsageOf(
  reading: SDKControlGetContextUsageResponse,
  options: { jevThreshold?: number | undefined; details?: boolean } = {},
): ContextUsage {
  const max = tokens(reading.rawMaxTokens || reading.maxTokens);
  const used = tokens(reading.totalTokens);
  const categories = (reading.categories ?? []).map((category) => ({
    name: category.name,
    tokens: tokens(category.tokens),
    kind: KINDS.includes(category.kind) ? category.kind : category.isDeferred ? ("deferred" as const) : ("used" as const),
  }));
  const usage: ContextUsage = {
    model: reading.model || null,
    used_tokens: used,
    max_tokens: max,
    percent: typeof reading.percentage === "number" ? reading.percentage : max > 0 ? Math.round((used / max) * 100) : 0,
    over_limit: max > 0 && used > max,
    categories,
    compaction: compactionOf(reading, max, options.jevThreshold),
  };
  if (options.details) usage.details = detailsOf(reading);
  return usage;
}

function compactionOf(reading: SDKControlGetContextUsageResponse, max: number, jevThreshold: number | undefined): ContextCompaction | null {
  if (jevThreshold !== undefined) return { by: "jev", threshold_percent: jevThreshold };
  if (!reading.isAutoCompactEnabled || max <= 0) return null;
  if (reading.autoCompactThreshold && reading.autoCompactThreshold > 0) {
    return { by: "claude_code", threshold_percent: clampPercent((reading.autoCompactThreshold / max) * 100) };
  }
  // Without a threshold, the autocompact buffer tells where it starts.
  const buffer = (reading.categories ?? []).filter((c) => c.kind === "buffer").reduce((sum, c) => sum + c.tokens, 0);
  return buffer > 0 && buffer < max ? { by: "claude_code", threshold_percent: clampPercent(((max - buffer) / max) * 100) } : null;
}

function detailsOf(reading: SDKControlGetContextUsageResponse): ContextDetails {
  return {
    mcp_tools: (reading.mcpTools ?? []).map((tool) => ({
      name: tool.name,
      server: tool.serverName,
      tokens: tokens(tool.tokens),
      loaded: tool.isLoaded ?? true,
    })),
    memory_files: (reading.memoryFiles ?? []).map((file) => ({ path: file.path, source: file.type, tokens: tokens(file.tokens) })),
    skills: (reading.skills?.skillFrontmatter ?? []).map((skill) => ({ name: skill.name, source: skill.source, tokens: tokens(skill.tokens) })),
    agents: (reading.agents ?? []).map((agent) => ({ name: agent.agentType, source: agent.source, tokens: tokens(agent.tokens) })),
  };
}

function tokens(value: number | undefined): number {
  return typeof value === "number" && Number.isFinite(value) && value > 0 ? Math.round(value) : 0;
}

function clampPercent(value: number): number {
  return Math.min(100, Math.max(0, Math.round(value * 10) / 10));
}
