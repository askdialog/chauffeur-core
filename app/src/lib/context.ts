// The context meter of a chat (composer footer, chat tab): how full the agent's context window is,
// by category, and where the chat compacts. Pure logic; ContextMeter.tsx draws it.

import type { ContextCategoryKind, ContextMcpTool, ContextUsage, ContextUsageResult } from "@chauffeur/protocol";

export type ContextTone = "neutral" | "warn" | "danger";

/** Points below the compaction threshold from which the meter turns amber. */
export const WARN_POINTS = 10;
/** Without a known threshold (Codex), where the meter turns amber and red. */
const WARN_WITHOUT_THRESHOLD = 80;
const DANGER_WITHOUT_THRESHOLD = 95;
/** The chat tab shows the share above this. */
export const TAB_PERCENT_FROM = 50;

/** Red past the compaction threshold or the window, amber within 10 points of it, else neutral. */
export function contextTone(usage: Pick<ContextUsage, "percent" | "over_limit" | "compaction">): ContextTone {
  if (usage.over_limit) return "danger";
  const threshold = usage.compaction?.threshold_percent;
  if (threshold == null) {
    if (usage.percent >= DANGER_WITHOUT_THRESHOLD) return "danger";
    return usage.percent >= WARN_WITHOUT_THRESHOLD ? "warn" : "neutral";
  }
  if (usage.percent >= threshold) return "danger";
  return usage.percent >= threshold - WARN_POINTS ? "warn" : "neutral";
}

/** The ring's fill, 0 to 1. */
export function ringFill(percent: number): number {
  return Math.max(0, Math.min(1, percent / 100));
}

/** `15%`, `0.4%` under one, `<0.1%` for a sliver. */
export function formatPercent(percent: number): string {
  if (percent <= 0) return "0%";
  if (percent < 0.1) return "<0.1%";
  if (percent < 1) return `${percent.toFixed(1)}%`;
  return `${Math.round(percent)}%`;
}

/** A row's share of the window, as Claude Code shows it: `11.3%`. */
export function formatShare(tokens: number, max: number): string {
  if (max <= 0) return "";
  const share = (tokens / max) * 100;
  if (share > 0 && share < 0.05) return "<0.1%";
  return `${share.toFixed(1)}%`;
}

/** `154.2k`, `1M`, `1.5M`, `812`. */
export function formatTokenCount(tokens: number): string {
  if (tokens >= 1_000_000) {
    const m = tokens / 1_000_000;
    return Number.isInteger(Math.round(m * 10) / 10) ? `${Math.round(m)}M` : `${m.toFixed(1)}M`;
  }
  if (tokens >= 1000) {
    const k = tokens / 1000;
    return Number.isInteger(Math.round(k * 10) / 10) ? `${Math.round(k)}k` : `${k.toFixed(1)}k`;
  }
  return String(Math.round(tokens));
}

/** The share in use: `15%`, `≈15%` for Chauffeur's estimate after a compaction. */
export function usagePercent(usage: Pick<ContextUsage, "percent" | "estimated">): string {
  return `${usage.estimated ? "≈" : ""}${formatPercent(usage.percent)}`;
}

/** `15% used · 154.2k / 1M`. */
export function usageHeadline(usage: Pick<ContextUsage, "percent" | "used_tokens" | "max_tokens" | "estimated">): string {
  return `${usagePercent(usage)} used · ${formatTokenCount(usage.used_tokens)} / ${formatTokenCount(usage.max_tokens)}`;
}

/** The chat tab's hint: the share once past half the window, else nothing. */
export function tabPercent(usage: ContextUsage | null | undefined): string | null {
  return usage && usage.percent > TAB_PERCENT_FROM ? usagePercent(usage) : null;
}

/** Colours by category, close to Claude Code's `/context`; grey for what is not the chat's own. */
const COLORS: Array<[RegExp, string]> = [
  [/^messages/i, "#4f8ef7"],
  [/^system tools/i, "#e5603e"],
  [/^skills/i, "#3fb37f"],
  [/^memory files/i, "#e0a03a"],
  [/^mcp tools/i, "#3bb4c9"],
  [/^(custom )?agents/i, "#a07ef0"],
  [/^slash commands/i, "#d06bb0"],
  [/^system prompt/i, "#8a8a93"],
  [/^mcp server instructions/i, "#6f6f78"],
];
const KIND_COLORS: Record<ContextCategoryKind, string> = {
  used: "#7c7c86",
  buffer: "#55555d",
  free: "#2e2e34",
  deferred: "#4a4a52",
};

export function categoryColor(name: string, kind: ContextCategoryKind): string {
  if (kind !== "used") return KIND_COLORS[kind];
  return COLORS.find(([pattern]) => pattern.test(name))?.[1] ?? KIND_COLORS.used;
}

export type ContextRow = {
  name: string;
  tokens: number;
  kind: ContextCategoryKind;
  color: string;
  /** Of the window; null for deferred rows, which are not in it. */
  share: string | null;
};

/**
 * The rows of the breakdown as Claude Code lists them: what uses the window, largest first, then
 * the buffer and the free space, then (apart) the deferred rows. Rows without tokens are hidden.
 */
export function contextRows(usage: Pick<ContextUsage, "categories" | "max_tokens">): { window: ContextRow[]; deferred: ContextRow[] } {
  const order: Record<ContextCategoryKind, number> = { used: 0, buffer: 1, free: 2, deferred: 3 };
  const rows = usage.categories
    .filter((category) => category.tokens > 0)
    .map((category, index) => ({ category, index }))
    .sort(
      (a, b) =>
        order[a.category.kind] - order[b.category.kind] ||
        (a.category.kind === "used" ? b.category.tokens - a.category.tokens : 0) ||
        a.index - b.index,
    )
    .map(({ category }) => ({
      name: category.name,
      tokens: category.tokens,
      kind: category.kind,
      color: categoryColor(category.name, category.kind),
      share: category.kind === "deferred" ? null : formatShare(category.tokens, usage.max_tokens),
    }));
  return { window: rows.filter((row) => row.kind !== "deferred"), deferred: rows.filter((row) => row.kind === "deferred") };
}

/** The stacked bar: the window's segments in order, widths in % of the window (free space left out). */
export function barSegments(usage: Pick<ContextUsage, "categories" | "max_tokens">): Array<{ name: string; color: string; width: number }> {
  if (usage.max_tokens <= 0) return [];
  return contextRows(usage)
    .window.filter((row) => row.kind === "used")
    .map((row) => ({ name: row.name, color: row.color, width: (row.tokens / usage.max_tokens) * 100 }));
}

export type McpServerGroup = {
  server: string;
  tokens: number;
  /** Tools whose schema is in the window; the others are deferred (loaded on demand). */
  loaded: number;
  tools: Array<{ name: string; tokens: number; loaded: boolean }>;
};

/** MCP tools by server, the heaviest server first, each with its tools heaviest first. */
export function mcpServers(tools: ContextMcpTool[]): McpServerGroup[] {
  const groups = new Map<string, McpServerGroup>();
  for (const tool of tools) {
    const group = groups.get(tool.server) ?? { server: tool.server, tokens: 0, loaded: 0, tools: [] };
    group.tokens += tool.tokens;
    if (tool.loaded) group.loaded++;
    group.tools.push({ name: mcpToolName(tool.name, tool.server), tokens: tool.tokens, loaded: tool.loaded });
    groups.set(tool.server, group);
  }
  const result = [...groups.values()];
  for (const group of result) group.tools.sort((a, b) => b.tokens - a.tokens || a.name.localeCompare(b.name));
  return result.sort((a, b) => b.tokens - a.tokens || a.server.localeCompare(b.server));
}

/** `mcp__linear__create_issue` → `create_issue`. */
export function mcpToolName(name: string, server: string): string {
  const prefix = `mcp__${server}__`;
  if (name.startsWith(prefix)) return name.slice(prefix.length);
  const parts = name.split("__");
  return parts.length >= 3 && parts[0] === "mcp" ? parts.slice(2).join("__") : name;
}

/**
 * A memory file's path as the user knows it: relative to the workspace (`web/CLAUDE.md`),
 * else to the home folder (`~/.claude/CLAUDE.md`). macOS's `/private` prefix (Claude Code resolves
 * `/tmp` and `/var` through it) does not stand in the way.
 */
export function shortPath(path: string, workspacePath: string | null | undefined, home?: string | null): string {
  const strip = (p: string) => p.replace(/^\/private(?=\/(tmp|var|etc)\/)/, "").replace(/\/+$/, "");
  const file = strip(path);
  if (workspacePath) {
    const base = strip(workspacePath);
    if (base && file.startsWith(`${base}/`)) return file.slice(base.length + 1);
  }
  const userHome = home ?? /^\/(Users|home)\/[^/]+/.exec(file)?.[0];
  if (userHome && file.startsWith(`${strip(userHome)}/`)) return `~${file.slice(strip(userHome).length)}`;
  return file;
}

/** `read at 10:31`, with the day when it is not today. */
export function readAtText(iso: string | null | undefined, now = new Date()): string | null {
  if (!iso) return null;
  const at = new Date(iso);
  if (Number.isNaN(at.getTime())) return null;
  const time = at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const sameDay = at.toDateString() === now.toDateString();
  return sameDay ? `read at ${time}` : `read ${at.toLocaleDateString([], { month: "short", day: "numeric" })} at ${time}`;
}

/** What compacts the chat, for the threshold's tooltip and the panel. */
export function compactionText(usage: Pick<ContextUsage, "compaction">): string | null {
  const compaction = usage.compaction;
  if (!compaction) return null;
  const at = formatPercent(compaction.threshold_percent);
  return compaction.by === "jev" ? `Jev compacts at ${at}` : `Claude Code compacts at ${at}`;
}

export type Reading = { usage: ContextUsage; readAt: string; live: boolean | null };

/**
 * The reading to show: the chat's last one (its events), unless the daemon answered one at least
 * as recent (`chat.context_usage`: the exact count, the compaction that applies now, whether the
 * session runs). On a tie the later argument wins: the full reading over the summary.
 */
export function freshestReading(events: { usage: ContextUsage; readAt: string }, ...answers: Array<ContextUsageResult | null>): Reading {
  let best: Reading = { ...events, live: null };
  for (const answer of answers) {
    if (!answer?.usage || !answer.read_at) continue;
    if (Date.parse(answer.read_at) >= Date.parse(best.readAt)) best = { usage: answer.usage, readAt: answer.read_at, live: answer.live };
  }
  return best;
}
