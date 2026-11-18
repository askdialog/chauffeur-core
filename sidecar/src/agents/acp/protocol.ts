// The part of the Agent Client Protocol (https://agentclientprotocol.com, protocol version 1) that
// Chauffeur speaks to an ACP agent, written by hand: the requests a client makes, the updates and
// requests an agent sends back. Fields Chauffeur does not read are left out; agents send more.

export const ACP_PROTOCOL_VERSION = 1;

export interface Implementation {
  name: string;
  title?: string | null;
  version: string;
}

export interface AuthMethod {
  id: string;
  name: string;
  description?: string | null;
}

export interface InitializeResponse {
  protocolVersion: number;
  agentCapabilities?: {
    loadSession?: boolean;
    promptCapabilities?: { image?: boolean; audio?: boolean; embeddedContext?: boolean };
    sessionCapabilities?: { list?: object | null; fork?: object | null; close?: object | null };
  };
  authMethods?: AuthMethod[];
  agentInfo?: Implementation | null;
}

/** An MCP server a session connects to. ACP agents must take stdio; HTTP is a capability. */
export type McpServerSpec =
  | { name: string; command: string; args: string[]; env: { name: string; value: string }[] }
  | { type: "http"; name: string; url: string; headers: { name: string; value: string }[] };

export interface SessionMode {
  id: string;
  name: string;
  description?: string | null;
}

export interface ConfigOptionValue {
  value: string;
  name: string;
  description?: string | null;
}

/** A session setting the agent lets the client change: its mode, its model, its effort… */
export interface ConfigOption {
  id: string;
  name: string;
  category?: string | null;
  type: "select";
  currentValue: string;
  options: ConfigOptionValue[];
}

export interface NewSessionResponse {
  sessionId: string;
  modes?: { currentModeId: string; availableModes: SessionMode[] } | null;
  configOptions?: ConfigOption[] | null;
  _meta?: Record<string, unknown> | null;
}

export type ContentBlock =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mimeType: string; uri?: string | null }
  | { type: "resource_link"; uri: string; name: string; mimeType?: string | null }
  | { type: "resource"; resource: { uri: string; text?: string; blob?: string; mimeType?: string | null } };

export type ToolKind = "read" | "edit" | "delete" | "move" | "search" | "execute" | "think" | "fetch" | "switch_mode" | "other";
export type ToolCallStatus = "pending" | "in_progress" | "completed" | "failed";

export type ToolCallContent =
  | { type: "content"; content: ContentBlock }
  | { type: "diff"; path: string; oldText?: string | null; newText: string }
  | { type: "terminal"; terminalId: string };

export interface ToolCallLocation {
  path: string;
  line?: number | null;
}

/** A tool call as `tool_call` announces it, and as `tool_call_update` changes it (all optional). */
export interface ToolCallFields {
  toolCallId: string;
  title?: string | null;
  kind?: ToolKind | null;
  status?: ToolCallStatus | null;
  content?: ToolCallContent[] | null;
  locations?: ToolCallLocation[] | null;
  rawInput?: unknown;
  rawOutput?: unknown;
}

export interface PlanEntry {
  content: string;
  priority?: string;
  status: "pending" | "in_progress" | "completed";
}

export type SessionUpdate =
  | { sessionUpdate: "user_message_chunk"; content: ContentBlock }
  | { sessionUpdate: "agent_message_chunk"; content: ContentBlock; messageId?: string | null }
  | { sessionUpdate: "agent_thought_chunk"; content: ContentBlock; messageId?: string | null }
  | ({ sessionUpdate: "tool_call" } & ToolCallFields)
  | ({ sessionUpdate: "tool_call_update" } & ToolCallFields)
  | { sessionUpdate: "plan"; entries: PlanEntry[] }
  | { sessionUpdate: "available_commands_update"; availableCommands: { name: string; description: string }[] }
  | { sessionUpdate: "current_mode_update"; currentModeId: string }
  | { sessionUpdate: "config_option_update"; configOptions: ConfigOption[] }
  | { sessionUpdate: "session_info_update"; title?: string | null }
  | { sessionUpdate: "usage_update"; used: number; size: number; _meta?: Record<string, unknown> | null };

export interface SessionNotification {
  sessionId: string;
  update: SessionUpdate;
}

export type StopReason = "end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled";

export interface PromptResponse {
  stopReason: StopReason;
  usage?: { totalTokens?: number; inputTokens?: number; outputTokens?: number; cachedReadTokens?: number } | null;
}

export type PermissionOptionKind = "allow_once" | "allow_always" | "reject_once" | "reject_always";

export interface PermissionOption {
  optionId: string;
  name: string;
  kind: PermissionOptionKind;
}

export interface RequestPermissionRequest {
  sessionId: string;
  /** Often only the call's id: the call itself came in `tool_call` before. */
  toolCall: ToolCallFields;
  options: PermissionOption[];
}

export type RequestPermissionOutcome = { outcome: "cancelled" } | { outcome: "selected"; optionId: string };
