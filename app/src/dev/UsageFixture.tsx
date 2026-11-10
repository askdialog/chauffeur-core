// Development only (`http://localhost:1420/#usage-fixture`): the sidebar's plan usage fed with
// fabricated readings (a window running out before it resets, extra usage, a failed read), and
// beside it an agent signed out and MCP servers lost, to work on its panel without a daemon. Not
// part of the production bundle.

import type { AgentKind, AgentUsage, ChatInfo, McpServer } from "@chauffeur/protocol";
import type { BrokenConnector } from "@/lib/connectors";
import { useState } from "react";
import { UsageMeterView } from "@/components/UsageMeter";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

function readings(now: number): AgentUsage[] {
  const at = (ms: number) => new Date(now + ms).toISOString();
  return [
    {
      agent: "claude",
      plan: "max",
      windows: [
        // 3h of 5 gone, 78% used: runs out in about 50 minutes.
        { label: "5h", used_percent: 78, resets_at: at(2 * HOUR + 2 * MIN), window_secs: 5 * 3600 },
        { label: "7d", used_percent: 41, resets_at: at(3 * DAY + 18 * HOUR), window_secs: 7 * 86400 },
        { label: "7d Fable", used_percent: 10, resets_at: at(3 * DAY + 18 * HOUR), window_secs: 7 * 86400 },
        { label: "7d Opus", used_percent: 3, resets_at: at(3 * DAY + 18 * HOUR), window_secs: 7 * 86400 },
      ],
      error: null,
      read_at: at(-3 * MIN),
      failed_at: null,
      extra_usage: { enabled: true, used: 1240, limit: 5000, currency: "USD" },
      account: "signed_in",
      account_error: null,
    },
    {
      agent: "codex",
      plan: "prolite",
      reset_credits: { available_count: 2, credits: [
        { title: "Full reset", description: null, expires_at: at(8 * DAY) },
        { title: "Full reset", description: null, expires_at: at(17 * DAY) },
      ] },
      windows: [{ label: "7d", used_percent: 0, resets_at: at(4 * DAY + 12 * HOUR), window_secs: 7 * 86400 }],
      error: "Codex could not read its usage: account/rateLimits/read: 401 Unauthorized (token expired; run `codex login`)",
      read_at: at(-42 * MIN),
      failed_at: at(-1 * MIN),
      extra_usage: null,
      account: "signed_in",
      account_error: null,
    },
  ];
}

const fixtureChat = (id: string, title: string, agent: AgentKind): ChatInfo => ({
  id,
  kind: "chat",
  workspace_id: "w",
  agent,
  model: null,
  effort: null,
  fast: false,
  permission_mode: "auto",
  title,
  status: "idle",
  agent_session_id: "s",
  created_at: "2026-10-18T10:00:00Z",
  running_tasks: 0,
  closed: false,
  last_activity_at: null,
  mcp_servers: [],
});

const lost = (name: string, status: McpServer["status"], error: string | null, failed_call: boolean, missing_env: string[] = []): McpServer => ({
  name,
  status,
  error,
  failed_call,
  missing_env,
  uses_env: missing_env,
  declared_in: missing_env.length > 0 ? `~/.codex/config.toml [mcp_servers.${name}]` : null,
});

const CONNECTORS: Record<AgentKind, BrokenConnector[]> = {
  claude: [
    { server: lost("claude.ai Linear", "needs_auth", null, true), chat: fixtureChat("c1", "Fix the empty cart", "claude") },
    { server: lost("playwright", "failed", "spawn npx ENOENT", false), chat: fixtureChat("c2", "Checkout QA", "claude") },
  ],
  codex: [
    {
      server: lost("slack", "failed", "MCP startup failed: Environment variable SLACK_MCP_TOKEN for MCP server 'slack' is not set", false, ["SLACK_MCP_TOKEN"]),
      chat: fixtureChat("c3", "Answer the support thread", "codex"),
    },
  ],
};

/** Codex signed out: its token was refused in a chat. */
function signedOut(now: number): AgentUsage[] {
  const [claude, codex] = readings(now);
  return [
    claude!,
    { ...codex!, error: null, failed_at: null, account: "signed_out", account_error: "Codex's login was refused (401 Unauthorized)" },
  ];
}

export function UsageFixture() {
  const [now, setNow] = useState(Date.now());
  const [reading, setReading] = useState(false);
  const refresh = () => {
    setReading(true);
    setTimeout(() => {
      setNow(Date.now());
      setReading(false);
    }, 800);
  };
  return (
    <div className="flex h-screen bg-bg text-fg">
      <aside className="flex w-[260px] flex-col border-r border-line bg-sidebar">
        <div className="flex-1 p-3 text-[12px] text-faint">Sidebar</div>
        <UsageMeterView agents={readings(now)} now={now} reading={reading} onRefresh={refresh} />
        <div className="h-9 border-t border-line" />
      </aside>
      <aside className="flex w-[260px] flex-col border-r border-line bg-sidebar" data-testid="fixture-lost">
        <div className="flex-1 p-3 text-[12px] text-faint">Signed out, MCP servers lost</div>
        <UsageMeterView
          agents={signedOut(now)}
          connectors={CONNECTORS}
          signingIn={[]}
          now={now}
          reading={reading}
          onRefresh={refresh}
          onSignIn={(agent) => console.log("sign in", agent)}
          onOpenChat={(chat) => console.log("open", chat.id)}
        />
        <div className="h-9 border-t border-line" />
      </aside>
      <main className="flex-1" />
    </div>
  );
}
