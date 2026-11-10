import { describe, expect, test } from "bun:test";
import type { ContextUsage } from "@chauffeur/protocol";
import {
  barSegments,
  compactionText,
  contextRows,
  contextTone,
  formatPercent,
  formatShare,
  formatTokenCount,
  freshestReading,
  mcpServers,
  mcpToolName,
  readAtText,
  ringFill,
  shortPath,
  tabPercent,
  usageHeadline,
} from "./context";

/** The reading of a user's screenshot: 15% of a 1M window. */
const USAGE: ContextUsage = {
  model: "claude-opus-5-5",
  used_tokens: 154_200,
  max_tokens: 1_000_000,
  percent: 15,
  over_limit: false,
  categories: [
    { name: "System prompt", tokens: 6_200, kind: "used" },
    { name: "System tools", tokens: 15_200, kind: "used" },
    { name: "MCP server instructions", tokens: 839, kind: "used" },
    { name: "MCP tools (deferred)", tokens: 3_400, kind: "deferred" },
    { name: "System tools (deferred)", tokens: 22_600, kind: "deferred" },
    { name: "Memory files", tokens: 9_200, kind: "used" },
    { name: "Skills", tokens: 9_900, kind: "used" },
    { name: "Custom agents", tokens: 0, kind: "used" },
    { name: "Messages", tokens: 112_800, kind: "used" },
    { name: "Autocompact buffer", tokens: 33_000, kind: "buffer" },
    { name: "Free space", tokens: 812_800, kind: "free" },
  ],
  compaction: { by: "claude_code", threshold_percent: 96.7 },
};

describe("the ring's colour", () => {
  const at = (percent: number, threshold: number | null, over_limit = false) =>
    contextTone({ percent, over_limit, compaction: threshold == null ? null : { by: "jev", threshold_percent: threshold } });

  test("neutral, amber within 10 points of the compaction threshold, red past it or over the window", () => {
    expect(at(15, 60)).toBe("neutral");
    expect(at(49, 60)).toBe("neutral");
    expect(at(50, 60)).toBe("warn");
    expect(at(59, 60)).toBe("warn");
    expect(at(60, 60)).toBe("danger");
    expect(at(20, 96.7, true)).toBe("danger");
  });

  test("without a known threshold (Codex): amber from 80%, red from 95%", () => {
    expect(at(79, null)).toBe("neutral");
    expect(at(80, null)).toBe("warn");
    expect(at(95, null)).toBe("danger");
  });

  test("fills with use, never past full", () => {
    expect(ringFill(15)).toBe(0.15);
    expect(ringFill(130)).toBe(1);
    expect(ringFill(-1)).toBe(0);
  });
});

describe("the breakdown", () => {
  test("rows as Claude Code lists them: used largest first, buffer, free, then the deferred apart; no zero rows", () => {
    const { window, deferred } = contextRows(USAGE);
    expect(window.map((r) => r.name)).toEqual([
      "Messages",
      "System tools",
      "Skills",
      "Memory files",
      "System prompt",
      "MCP server instructions",
      "Autocompact buffer",
      "Free space",
    ]);
    expect(deferred.map((r) => r.name)).toEqual(["MCP tools (deferred)", "System tools (deferred)"]);
    expect(window[0]).toMatchObject({ tokens: 112_800, share: "11.3%", kind: "used" });
    expect(window.at(-1)).toMatchObject({ share: "81.3%", kind: "free" });
    // Deferred schemas are not in the window: no share.
    expect(deferred[0]?.share).toBeNull();
    // Each category its own colour; what is not the chat's own in grey.
    expect(new Set(window.slice(0, 4).map((r) => r.color)).size).toBe(4);
  });

  test("the stacked bar holds what uses the window, in the rows' order", () => {
    const segments = barSegments(USAGE);
    expect(segments.map((s) => s.name)).toEqual(["Messages", "System tools", "Skills", "Memory files", "System prompt", "MCP server instructions"]);
    expect(segments[0]?.width).toBeCloseTo(11.28);
    expect(barSegments({ ...USAGE, max_tokens: 0 })).toEqual([]);
  });

  test("Codex: no categories, no rows", () => {
    expect(contextRows({ categories: [], max_tokens: 400_000 })).toEqual({ window: [], deferred: [] });
  });
});

describe("MCP tools by server", () => {
  test("servers heaviest first, with their total, tools heaviest first and named without their prefix", () => {
    const groups = mcpServers([
      { name: "mcp__linear__get_issue", server: "linear", tokens: 300, loaded: false },
      { name: "mcp__claap__list_users", server: "claap", tokens: 900, loaded: true },
      { name: "mcp__linear__save_issue", server: "linear", tokens: 1_200, loaded: false },
    ]);
    expect(groups).toEqual([
      {
        server: "linear",
        tokens: 1_500,
        loaded: 0,
        tools: [
          { name: "save_issue", tokens: 1_200, loaded: false },
          { name: "get_issue", tokens: 300, loaded: false },
        ],
      },
      { server: "claap", tokens: 900, loaded: 1, tools: [{ name: "list_users", tokens: 900, loaded: true }] },
    ]);
  });

  test("a server name with underscores, or a tool outside the convention", () => {
    expect(mcpToolName("mcp__claude_ai_Linear__get_issue", "claude_ai_Linear")).toBe("get_issue");
    expect(mcpToolName("mcp__plugin_x__srv__do_it", "other")).toBe("srv__do_it");
    expect(mcpToolName("search", "s")).toBe("search");
  });
});

describe("paths and numbers", () => {
  test("a memory file relative to the workspace, else to the home folder", () => {
    const ws = "/Users/alice/chauffeur/workspaces/xyz-487";
    expect(shortPath(`${ws}/web/CLAUDE.md`, ws)).toBe("web/CLAUDE.md");
    expect(shortPath(`${ws}/CLAUDE.md`, `${ws}/`)).toBe("CLAUDE.md");
    expect(shortPath("/Users/alice/.claude/CLAUDE.md", ws)).toBe("~/.claude/CLAUDE.md");
    expect(shortPath("/home/ana/.claude/CLAUDE.md", null)).toBe("~/.claude/CLAUDE.md");
    // Claude Code resolves /tmp through macOS's /private.
    expect(shortPath("/private/tmp/ws/CLAUDE.md", "/tmp/ws")).toBe("CLAUDE.md");
    expect(shortPath("/etc/claude/CLAUDE.md", ws)).toBe("/etc/claude/CLAUDE.md");
  });

  test("tokens, shares and the headline as Claude Code writes them", () => {
    expect(formatTokenCount(154_200)).toBe("154.2k");
    expect(formatTokenCount(33_000)).toBe("33k");
    expect(formatTokenCount(1_000_000)).toBe("1M");
    expect(formatTokenCount(1_500_000)).toBe("1.5M");
    expect(formatTokenCount(839)).toBe("839");
    expect(formatShare(839, 1_000_000)).toBe("0.1%");
    expect(formatShare(14, 1_000_000)).toBe("<0.1%");
    expect(formatPercent(15)).toBe("15%");
    expect(formatPercent(0.4)).toBe("0.4%");
    expect(usageHeadline(USAGE)).toBe("15% used · 154.2k / 1M");
    // Chauffeur's estimate after a compaction, until the agent measures it.
    expect(usageHeadline({ ...USAGE, estimated: true })).toBe("≈15% used · 154.2k / 1M");
  });

  test("the chat tab says how full the context is past half the window only", () => {
    expect(tabPercent({ ...USAGE, percent: 50 })).toBeNull();
    expect(tabPercent({ ...USAGE, percent: 63 })).toBe("63%");
    expect(tabPercent({ ...USAGE, percent: 63, estimated: true })).toBe("≈63%");
    expect(tabPercent(null)).toBeNull();
  });

  test("when it was read, and what compacts the chat", () => {
    const now = new Date(2026, 9, 18, 12, 0);
    expect(readAtText(new Date(2026, 9, 18, 10, 31).toISOString(), now)).toMatch(/^read at 10:31/);
    expect(readAtText(new Date(2026, 9, 17, 10, 31).toISOString(), now)).toMatch(/^read Oct 17 at 10:31/);
    expect(readAtText(null)).toBeNull();
    expect(compactionText(USAGE)).toBe("Claude Code compacts at 97%");
    expect(compactionText({ compaction: { by: "jev", threshold_percent: 60 } })).toBe("Jev compacts at 60%");
    expect(compactionText({ compaction: null })).toBeNull();
  });
});

describe("the reading shown", () => {
  const at = (iso: string, percent: number, live = true) => ({ usage: { ...USAGE, percent }, read_at: iso, live });
  test("the daemon's answer when at least as recent as the chat's events, the full one on a tie", () => {
    const events = { usage: USAGE, readAt: "2026-10-18T10:00:00.000Z" };
    expect(freshestReading(events, null, null)).toEqual({ ...events, live: null });
    // Older than the last turn's reading: ignored.
    expect(freshestReading(events, at("2026-10-18T09:59:00.000Z", 9)).usage.percent).toBe(15);
    // The stored reading answered for a closed session: same time, its compaction and `live`.
    expect(freshestReading(events, at("2026-10-18T10:00:00.000Z", 15, false)).live).toBe(false);
    expect(freshestReading(events, at("2026-10-18T10:01:00.000Z", 16), at("2026-10-18T10:01:00.000Z", 17)).usage.percent).toBe(17);
    expect(freshestReading(events, at("2026-10-18T10:02:00.000Z", 16), at("2026-10-18T10:01:00.000Z", 17)).usage.percent).toBe(16);
  });
});
