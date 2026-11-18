// A stand-in for an ACP agent process (`vibe-acp`): answers initialize and the session's set-up,
// records every request, and lets the test end prompts, send updates and ask permissions.

import type { AcpConnection, AcpHandlers, AcpSpawnOptions } from "../../src/agents/acp/connection.ts";
import type { ConfigOption, PromptResponse, RequestPermissionOutcome, SessionUpdate } from "../../src/agents/acp/protocol.ts";

export const SESSION_ID = "vibe-session-1";

export const CONFIG_OPTIONS: ConfigOption[] = [
  {
    id: "model",
    name: "Model",
    category: "model",
    type: "select",
    currentValue: "mistral-medium-latest",
    options: [
      { value: "mistral-large-4-0", name: "Mistral Large 4", description: "mistral-large-4-0" },
      { value: "mistral-medium-latest", name: "Mistral Medium", description: "mistral-medium-latest" },
    ],
  },
  {
    id: "thinking",
    name: "Thinking",
    category: "thinking",
    type: "select",
    currentValue: "high",
    options: [
      { value: "low", name: "Low" },
      { value: "medium", name: "Medium" },
      { value: "high", name: "High" },
    ],
  },
];

export class FakeAcp implements AcpConnection {
  readonly pid = 4242;
  readonly requests: { method: string; params: unknown }[] = [];
  readonly notifications: { method: string; params: unknown }[] = [];
  readonly handlers: AcpHandlers;
  readonly options: AcpSpawnOptions;
  /** The prompts running, oldest first: `endPrompt` answers the first. */
  private readonly prompts: ((response: PromptResponse) => void)[] = [];
  private exit!: (reason: string) => void;
  readonly exited = new Promise<string>((resolve) => (this.exit = resolve));
  closed = false;
  /** A failure for a method, instead of its answer. */
  failures = new Map<string, Error>();
  /** Whether Vibe trusts the session's folder, as `session/new` says until a trust decision. */
  trusted = true;
  private sessions = 0;

  constructor(options: AcpSpawnOptions) {
    this.options = options;
    this.handlers = options.handlers;
  }

  async request<T>(method: string, params: unknown): Promise<T> {
    this.requests.push({ method, params });
    const failure = this.failures.get(method);
    if (failure) throw failure;
    switch (method) {
      case "initialize":
        return { protocolVersion: 1, agentInfo: { name: "@mistralai/mistral-vibe", version: "3.1.3" } } as T;
      case "session/new":
      case "session/load": {
        const sessionId = method === "session/new" && this.sessions++ > 0 ? `${SESSION_ID}-${this.sessions}` : SESSION_ID;
        const _meta = { workspace_trust: { status: this.trusted ? "trusted" : "untrusted" } };
        return { sessionId, modes: { currentModeId: "accept-edits", availableModes: [] }, configOptions: CONFIG_OPTIONS, _meta } as T;
      }
      case "_trust/decision":
        this.trusted = true;
        return { trust_status: "trusted" } as T;
      case "session/prompt":
        return new Promise<T>((resolve) => this.prompts.push(resolve as (response: PromptResponse) => void));
      default:
        return {} as T;
    }
  }

  notify(method: string, params: unknown): void {
    this.notifications.push({ method, params });
  }

  async close(): Promise<void> {
    this.closed = true;
    this.exit("exited with code 0");
  }

  /** Vibe's process dies on its own. */
  crash(reason = "exited with code 1"): void {
    this.exit(reason);
  }

  update(update: SessionUpdate, sessionId = SESSION_ID): void {
    this.handlers.notification("session/update", { sessionId, update });
  }

  /** Vibe asks for a permission on a call it announced. */
  askPermission(toolCallId: string): Promise<{ outcome: RequestPermissionOutcome }> {
    return this.handlers.request(
      "session/request_permission",
      {
        sessionId: SESSION_ID,
        toolCall: { toolCallId },
        options: [
          { optionId: "allow_once", name: "Allow once", kind: "allow_once" },
          { optionId: "allow_always", name: "Allow for this session", kind: "allow_always" },
          { optionId: "reject_once", name: "Deny", kind: "reject_once" },
        ],
      },
      1,
    ) as Promise<{ outcome: RequestPermissionOutcome }>;
  }

  endPrompt(response: PromptResponse = { stopReason: "end_turn", usage: { totalTokens: 1200 } }): void {
    this.prompts.shift()?.(response);
  }

  get running(): number {
    return this.prompts.length;
  }

  sent(method: string): unknown[] {
    return this.requests.filter((request) => request.method === method).map((request) => request.params);
  }
}
