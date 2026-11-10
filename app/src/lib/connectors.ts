// Whether the agents' logins and their MCP servers hold: what the sidebar's foot, a chat's
// composer and the notifications say when one is lost (decided with Justin 2026-10-18).

import type { AgentKind, AgentUsage, ChatInfo, EnvReloadResult, McpServer } from "@chauffeur/protocol";

export function isBroken(server: McpServer): boolean {
  return server.status === "needs_auth" || server.status === "failed";
}

/** A broken server that lacks environment variables: guided to set them, not shown an error. */
export function lacksEnv(server: McpServer): boolean {
  return isBroken(server) && server.missing_env.length > 0;
}

/** Where a server stands, in a few words. */
export function mcpStatusText(server: McpServer): string {
  if (lacksEnv(server)) return `Needs ${names(server.missing_env)}`;
  switch (server.status) {
    case "connected":
      return "Connected";
    case "pending":
      return "Connecting…";
    case "needs_auth":
      return "Needs you to log in again";
    case "failed":
      return server.error ? `Failed: ${server.error}` : "Failed to connect";
    case "disabled":
      return "Turned off";
  }
}

/** The servers broken first, then those connecting, connected, and turned off; by name within. */
export function sortedServers(servers: McpServer[]): McpServer[] {
  const order: Record<McpServer["status"], number> = { needs_auth: 0, failed: 0, pending: 1, connected: 2, disabled: 3 };
  const rank = (server: McpServer) => order[server.status];
  return [...servers].sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
}

/** Servers a tool call needed while they were broken, that were not before. */
export function newlyNeeded(before: McpServer[] | undefined, after: McpServer[]): McpServer[] {
  const needed = new Set((before ?? []).filter((server) => server.failed_call).map((server) => server.name));
  return after.filter((server) => server.failed_call && !needed.has(server.name));
}

export type BrokenConnector = { server: McpServer; chat: ChatInfo };

/** An agent's broken servers across its chats running now, each once (the chat that needs it first). */
export function brokenConnectors(chats: ChatInfo[], agent: AgentKind): BrokenConnector[] {
  const found = new Map<string, BrokenConnector>();
  for (const chat of chats) {
    if (chat.agent !== agent || chat.closed) continue;
    for (const server of chat.mcp_servers) {
      if (!isBroken(server)) continue;
      const known = found.get(server.name);
      if (!known || (server.failed_call && !known.server.failed_call)) found.set(server.name, { server, chat });
    }
  }
  return [...found.values()].sort((a, b) => a.server.name.localeCompare(b.server.name));
}

/** The command that logs an agent in from a terminal, when Chauffeur cannot do it itself. */
export function loginCommand(agent: AgentKind): string {
  return agent === "claude" ? "claude auth login" : "codex login";
}

/** The command that logs an agent's MCP server in from a terminal opened in its workspace. */
export function mcpLoginCommand(agent: AgentKind, server: string): string {
  return agent === "claude" ? "claude /mcp" : `codex mcp login ${shellQuote(server)}`;
}

function shellQuote(value: string): string {
  return /^[A-Za-z0-9_.-]+$/.test(value) ? value : `'${value.replace(/'/g, `'\\''`)}'`;
}

/** `A`, `A and B`, `A, B and C`. */
function names(items: string[]): string {
  return items.length <= 1 ? (items[0] ?? "") : `${items.slice(0, -1).join(", ")} and ${items.at(-1)}`;
}

/** The line that sets a variable in the login shell's profile; the user types the value. */
export function exportLine(variable: string): string {
  return `export ${variable}=…`;
}

/** Where to export a variable: the daemon's login shell's file, when the daemon said. */
export function profileName(profile: string | undefined): string {
  return profile || "your shell's profile";
}

/** What reloading the environment did, in one line. */
export function reloadSummary(result: EnvReloadResult): { message: string; level: "info" | "error" } {
  if (result.still_missing.length > 0) {
    const verb = result.still_missing.length === 1 ? "is" : "are";
    return { message: `${names(result.still_missing)} ${verb} still not set in ${result.profile}'s environment`, level: "error" };
  }
  const changed = result.added.length + result.changed.length + result.removed.length;
  if (changed === 0) return { message: `Environment reloaded: nothing changed in ${result.profile}`, level: "info" };
  const parts = [`Environment reloaded (${changed} variable${changed === 1 ? "" : "s"} changed)`];
  if (result.restarted.length > 0) {
    parts.push(`restarting ${result.restarted.length} chat${result.restarted.length === 1 ? "" : "s"} for ${result.restarted.length === 1 ? "its" : "their"} MCP servers`);
  }
  if (result.deferred.length > 0) {
    const n = result.deferred.length;
    parts.push(`${n} working chat${n === 1 ? " gets" : "s get"} it at ${n === 1 ? "its" : "their"} next message`);
  }
  return { message: parts.join(": "), level: "info" };
}

/** A server whose chat restarted with the new environment, until it comes up or fails again. */
export type AwaitedServer = { chatId: string; server: string };

/**
 * The servers a reload restarted chats for: those broken in the chats it restarted, as they were
 * before (`before`: the chats then; a restarted session reports its servers anew), that take a
 * value from a variable it set or changed. Another broken server of the chat is not awaited.
 */
export function awaitedAfterReload(result: EnvReloadResult, before: Record<string, ChatInfo>): AwaitedServer[] {
  const set = new Set([...result.added, ...result.changed]);
  const takesOne = (server: McpServer) => [...server.uses_env, ...server.missing_env].some((name) => set.has(name));
  return result.restarted.flatMap((chatId) =>
    (before[chatId]?.mcp_servers ?? []).filter((server) => isBroken(server) && takesOne(server)).map((server) => ({ chatId, server: server.name })),
  );
}

/**
 * Where the awaited servers of a chat stand now: connected, or broken again; one still starting
 * (or not reported yet by the new session) stays awaited.
 */
export function settledServers(awaited: AwaitedServer[], chat: ChatInfo): { connected: string[]; broken: McpServer[]; rest: AwaitedServer[] } {
  const connected: string[] = [];
  const broken: McpServer[] = [];
  const rest = awaited.filter((entry) => {
    if (entry.chatId !== chat.id) return true;
    const server = chat.mcp_servers.find((s) => s.name === entry.server);
    if (server?.status === "connected") connected.push(server.name);
    else if (server && isBroken(server)) broken.push(server);
    else return true;
    return false;
  });
  return { connected, broken, rest };
}

/** An agent that went from signed in (or unknown) to signed out between two readings. */
export function signedOutSince(before: AgentUsage[] | null, after: AgentUsage[]): AgentUsage[] {
  if (before === null) return [];
  return after.filter((usage) => {
    if (usage.account !== "signed_out") return false;
    const was = before.find((b) => b.agent === usage.agent);
    return was !== undefined && was.account !== "signed_out";
  });
}
