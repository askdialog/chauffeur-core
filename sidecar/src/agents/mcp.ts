// Where a session's MCP servers stand, for both agents: each agent reads its servers its own way
// (Claude Code's `mcpServerStatus`, Codex's `mcpServerStatus/list` and startup notifications) and
// hands them here; the tracker tells the daemon when they change (`mcp_status`), notes the servers
// a tool call needed while they were broken (`failed_call`, what the app notifies about), and
// watches a login the user finishes in a browser until the server connects.

import type { ChatEvent, McpServer, McpStatus } from "../protocol.ts";

const BROKEN: ReadonlySet<McpStatus> = new Set(["needs_auth", "failed"]);

/** How often servers still connecting are read again, and for how long after the session starts. */
const PENDING_POLL_MS = 3_000;
const PENDING_POLL_FOR_MS = 60_000;
/** How often a server whose login was started is read again, and for how long. */
const LOGIN_POLL_MS = 3_000;
const LOGIN_POLL_FOR_MS = 5 * 60_000;

export interface McpTrackerDeps {
  emit: (event: ChatEvent) => void;
  /** The servers as the agent reports them now. */
  read: () => Promise<McpServer[]>;
  log?: (message: string) => void;
  /** Overridable in tests. */
  setTimeout?: (fn: () => void, ms: number) => unknown;
  clearTimeout?: (handle: unknown) => void;
  now?: () => number;
}

/** The part of a tool's name that names its server: `mcp__claude_ai_Linear__list_issues` → `claude_ai_Linear`. */
export function mcpToolServer(toolName: string): string | undefined {
  const match = /^mcp__(.+?)__/.exec(toolName);
  return match?.[1];
}

/** Server names as they appear in tool names: characters outside `[A-Za-z0-9_-]` become `_`. */
export function normalizeServerName(name: string): string {
  return name.replace(/[^A-Za-z0-9_-]/g, "_");
}

export class McpTracker {
  private servers: McpServer[] = [];
  private needed = new Set<string>();
  /** MCP tool calls in flight: tool_use_id → server name as in the tool's name. */
  private calls = new Map<string, string>();
  private reading: Promise<void> | undefined;
  private pendingPoll: unknown;
  private pendingUntil = 0;
  private logins = new Map<string, { until: number; timer: unknown; tick?: () => Promise<void> }>();
  private stopped = false;

  constructor(private readonly deps: McpTrackerDeps) {}

  /** The servers last read. */
  current(): McpServer[] {
    return this.servers;
  }

  find(name: string): McpServer | undefined {
    return this.servers.find((server) => server.name === name);
  }

  /** Starts following the session's servers: read now, and again while some are still connecting. */
  start(): void {
    this.pendingUntil = this.now() + PENDING_POLL_FOR_MS;
    void this.refresh();
  }

  /** Reads the servers again (one read at a time; a read asked meanwhile joins it). */
  refresh(): Promise<void> {
    if (this.stopped) return Promise.resolve();
    this.reading ??= this.deps
      .read()
      .then((servers) => this.update(servers))
      .catch((error: unknown) => this.deps.log?.(`could not read the MCP servers: ${String(error)}`))
      .finally(() => {
        this.reading = undefined;
      });
    return this.reading;
  }

  /** Takes the servers as read, tells the daemon if anything changed. */
  update(servers: McpServer[]): void {
    if (this.stopped) return;
    for (const server of servers) {
      // Needed while it stays broken; a server that connects again is fine again.
      if (!BROKEN.has(server.status)) this.needed.delete(server.name);
    }
    const next = servers.map((server) => ({ ...server, failed_call: this.needed.has(server.name) }));
    const changed = JSON.stringify(next) !== JSON.stringify(this.servers);
    this.servers = next;
    if (changed) this.deps.emit({ type: "mcp_status", servers: next });
    for (const name of this.logins.keys()) {
      if (this.find(name)?.status === "connected") this.endLogin(name);
    }
    this.schedulePendingPoll();
  }

  /**
   * Follows the session's events: an MCP tool call that fails, or that a broken server got, reads
   * the servers again and marks the server needed while it stays broken.
   */
  observe(event: ChatEvent): void {
    if (event.type === "tool_call") {
      const server = mcpToolServer(event.name);
      if (server) this.calls.set(event.tool_use_id, server);
      return;
    }
    if (event.type !== "tool_result") return;
    const tool = this.calls.get(event.tool_use_id);
    if (tool === undefined) return;
    this.calls.delete(event.tool_use_id);
    const known = this.byToolName(tool);
    if (!event.is_error && !(known && BROKEN.has(known.status))) return;
    void this.refresh().then(() => {
      const server = this.byToolName(tool);
      if (!server || !BROKEN.has(server.status) || this.needed.has(server.name)) return;
      this.needed.add(server.name);
      this.update(this.servers);
    });
  }

  /**
   * The user was sent to log a server in: read it again every few seconds until it connects (or
   * after a few minutes). `tick` runs before each read (Claude asks claude.ai connectors to
   * reconnect, which has no callback of its own).
   */
  watchLogin(name: string, tick?: () => Promise<void>): void {
    this.endLogin(name);
    const login = { until: this.now() + LOGIN_POLL_FOR_MS, timer: undefined as unknown, tick };
    this.logins.set(name, login);
    const poll = () => {
      if (this.stopped || this.logins.get(name) !== login) return;
      if (this.now() >= login.until) return void this.logins.delete(name);
      login.timer = this.schedule(async () => {
        await login.tick?.().catch((error: unknown) => this.deps.log?.(`reconnecting ${name}: ${String(error)}`));
        await this.refresh();
        poll();
      }, LOGIN_POLL_MS);
    };
    poll();
  }

  stop(): void {
    this.stopped = true;
    if (this.pendingPoll !== undefined) this.unschedule(this.pendingPoll);
    for (const name of [...this.logins.keys()]) this.endLogin(name);
  }

  private endLogin(name: string): void {
    const login = this.logins.get(name);
    if (!login) return;
    if (login.timer !== undefined) this.unschedule(login.timer);
    this.logins.delete(name);
  }

  private byToolName(tool: string): McpServer | undefined {
    return this.servers.find((server) => normalizeServerName(server.name) === tool);
  }

  private schedulePendingPoll(): void {
    if (this.pendingPoll !== undefined || this.stopped) return;
    if (!this.servers.some((server) => server.status === "pending") || this.now() >= this.pendingUntil) return;
    this.pendingPoll = this.schedule(() => {
      this.pendingPoll = undefined;
      void this.refresh();
    }, PENDING_POLL_MS);
  }

  private schedule(fn: () => void | Promise<void>, ms: number): unknown {
    const set = this.deps.setTimeout ?? ((f: () => void, delay: number) => setTimeout(f, delay));
    return set(() => void fn(), ms);
  }

  private unschedule(handle: unknown): void {
    const clear = this.deps.clearTimeout ?? ((h: unknown) => clearTimeout(h as ReturnType<typeof setTimeout>));
    clear(handle);
  }

  private now(): number {
    return (this.deps.now ?? Date.now)();
  }
}
