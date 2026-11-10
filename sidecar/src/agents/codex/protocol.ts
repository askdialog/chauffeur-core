// The part of the Codex app-server protocol the sidecar uses. Types come from
// `codex app-server generate-ts` (stable surface only, no --experimental, but for the one field
// named below) in ./generated, produced
// by codex-cli CODEX_APP_SERVER_VERSION (`bun run gen:codex` regenerates them).

import type { ClientInfo } from "./generated/ClientInfo.ts";
import type { InitializeCapabilities } from "./generated/InitializeCapabilities.ts";
import type { InitializeParams } from "./generated/InitializeParams.ts";
import type { InitializeResponse } from "./generated/InitializeResponse.ts";
import type { ServerNotification } from "./generated/ServerNotification.ts";
import type { ServerRequest } from "./generated/ServerRequest.ts";
import type { CancelLoginAccountParams } from "./generated/v2/CancelLoginAccountParams.ts";
import type { CancelLoginAccountResponse } from "./generated/v2/CancelLoginAccountResponse.ts";
import type { ConfigReadParams } from "./generated/v2/ConfigReadParams.ts";
import type { ConfigReadResponse } from "./generated/v2/ConfigReadResponse.ts";
import type { GetAccountParams } from "./generated/v2/GetAccountParams.ts";
import type { GetAccountResponse } from "./generated/v2/GetAccountResponse.ts";
import type { ListMcpServerStatusParams } from "./generated/v2/ListMcpServerStatusParams.ts";
import type { ListMcpServerStatusResponse } from "./generated/v2/ListMcpServerStatusResponse.ts";
import type { LoginAccountParams } from "./generated/v2/LoginAccountParams.ts";
import type { LoginAccountResponse } from "./generated/v2/LoginAccountResponse.ts";
import type { McpServerOauthLoginParams } from "./generated/v2/McpServerOauthLoginParams.ts";
import type { McpServerOauthLoginResponse } from "./generated/v2/McpServerOauthLoginResponse.ts";
import type { McpServerRefreshResponse } from "./generated/v2/McpServerRefreshResponse.ts";
import type { GetAccountRateLimitsParams } from "./generated/v2/GetAccountRateLimitsParams.ts";
import type { GetAccountRateLimitsResponse } from "./generated/v2/GetAccountRateLimitsResponse.ts";
import type { HooksListParams } from "./generated/v2/HooksListParams.ts";
import type { HooksListResponse } from "./generated/v2/HooksListResponse.ts";
import type { ModelListParams } from "./generated/v2/ModelListParams.ts";
import type { ModelListResponse } from "./generated/v2/ModelListResponse.ts";
import type { SkillsExtraRootsSetParams } from "./generated/v2/SkillsExtraRootsSetParams.ts";
import type { SkillsExtraRootsSetResponse } from "./generated/v2/SkillsExtraRootsSetResponse.ts";
import type { SkillsListParams } from "./generated/v2/SkillsListParams.ts";
import type { SkillsListResponse } from "./generated/v2/SkillsListResponse.ts";
import type { ThreadResumeParams } from "./generated/v2/ThreadResumeParams.ts";
import type { ThreadResumeResponse } from "./generated/v2/ThreadResumeResponse.ts";
import type { ThreadCompactStartParams } from "./generated/v2/ThreadCompactStartParams.ts";
import type { ThreadCompactStartResponse } from "./generated/v2/ThreadCompactStartResponse.ts";
import type { ThreadForkParams } from "./generated/v2/ThreadForkParams.ts";
import type { ThreadInjectItemsParams } from "./generated/v2/ThreadInjectItemsParams.ts";
import type { ThreadInjectItemsResponse } from "./generated/v2/ThreadInjectItemsResponse.ts";
import type { ThreadReadParams } from "./generated/v2/ThreadReadParams.ts";
import type { ThreadReadResponse } from "./generated/v2/ThreadReadResponse.ts";
import type { ThreadForkResponse } from "./generated/v2/ThreadForkResponse.ts";
import type { ThreadStartParams } from "./generated/v2/ThreadStartParams.ts";
import type { ThreadStartResponse } from "./generated/v2/ThreadStartResponse.ts";
import type { DynamicToolSpec } from "./generated/v2/DynamicToolSpec.ts";
import type { TurnInterruptParams } from "./generated/v2/TurnInterruptParams.ts";
import type { TurnInterruptResponse } from "./generated/v2/TurnInterruptResponse.ts";
import type { TurnStartParams } from "./generated/v2/TurnStartParams.ts";
import type { TurnStartResponse } from "./generated/v2/TurnStartResponse.ts";
import type { TurnSteerParams } from "./generated/v2/TurnSteerParams.ts";
import type { TurnSteerResponse } from "./generated/v2/TurnSteerResponse.ts";

export type { ClientInfo, InitializeCapabilities, ServerNotification, ServerRequest };
export type { AskForApproval } from "./generated/v2/AskForApproval.ts";
export type { ApprovalsReviewer } from "./generated/v2/ApprovalsReviewer.ts";
export type { CommandExecutionApprovalDecision } from "./generated/v2/CommandExecutionApprovalDecision.ts";
export type { CommandExecutionRequestApprovalParams } from "./generated/v2/CommandExecutionRequestApprovalParams.ts";
export type { FileChangeApprovalDecision } from "./generated/v2/FileChangeApprovalDecision.ts";
export type { ToolRequestUserInputResponse } from "./generated/v2/ToolRequestUserInputResponse.ts";
export type { SkillMetadata } from "./generated/v2/SkillMetadata.ts";
export type { FileChangeRequestApprovalParams } from "./generated/v2/FileChangeRequestApprovalParams.ts";
export type { FileUpdateChange } from "./generated/v2/FileUpdateChange.ts";
export type { Model } from "./generated/v2/Model.ts";
export type { PermissionsRequestApprovalParams } from "./generated/v2/PermissionsRequestApprovalParams.ts";
export type { PermissionsRequestApprovalResponse } from "./generated/v2/PermissionsRequestApprovalResponse.ts";
export type { SandboxMode } from "./generated/v2/SandboxMode.ts";
export type { SandboxPolicy } from "./generated/v2/SandboxPolicy.ts";
export type { ThreadItem } from "./generated/v2/ThreadItem.ts";
export type { TokenUsageBreakdown } from "./generated/v2/TokenUsageBreakdown.ts";
export type { Turn } from "./generated/v2/Turn.ts";
export type { UserInput } from "./generated/v2/UserInput.ts";
export type { McpServerStatus } from "./generated/v2/McpServerStatus.ts";
export type { SkillsListResponse, ThreadStartParams, ThreadResumeParams, TurnStartParams };
export type { DynamicToolCallParams } from "./generated/v2/DynamicToolCallParams.ts";
export type { DynamicToolCallResponse } from "./generated/v2/DynamicToolCallResponse.ts";
export type { DynamicToolSpec };

/**
 * `thread/start` with its `dynamicTools`, a field of Codex's experimental API (left out of the
 * stable types): the tools the thread offers its agent, kept when the thread is resumed.
 */
export type ThreadStartWithToolsParams = ThreadStartParams & { dynamicTools?: DynamicToolSpec[] | null };

/** Client → app-server requests the sidecar sends: `[params, result]`. */
export type CodexMethods = {
  initialize: [InitializeParams, InitializeResponse];
  "account/rateLimits/read": [GetAccountRateLimitsParams, GetAccountRateLimitsResponse];
  "account/read": [GetAccountParams, GetAccountResponse];
  "account/login/start": [LoginAccountParams, LoginAccountResponse];
  "account/login/cancel": [CancelLoginAccountParams, CancelLoginAccountResponse];
  "mcpServerStatus/list": [ListMcpServerStatusParams, ListMcpServerStatusResponse];
  "mcpServer/oauth/login": [McpServerOauthLoginParams, McpServerOauthLoginResponse];
  "config/mcpServer/reload": [undefined, McpServerRefreshResponse];
  "config/read": [ConfigReadParams, ConfigReadResponse];
  "hooks/list": [HooksListParams, HooksListResponse];
  "model/list": [ModelListParams, ModelListResponse];
  "skills/list": [SkillsListParams, SkillsListResponse];
  "skills/extraRoots/set": [SkillsExtraRootsSetParams, SkillsExtraRootsSetResponse];
  "thread/start": [ThreadStartWithToolsParams, ThreadStartResponse];
  "thread/resume": [ThreadResumeParams, ThreadResumeResponse];
  "thread/fork": [ThreadForkParams, ThreadForkResponse];
  "thread/compact/start": [ThreadCompactStartParams, ThreadCompactStartResponse];
  "thread/read": [ThreadReadParams, ThreadReadResponse];
  "thread/inject_items": [ThreadInjectItemsParams, ThreadInjectItemsResponse];
  "turn/start": [TurnStartParams, TurnStartResponse];
  "turn/steer": [TurnSteerParams, TurnSteerResponse];
  "turn/interrupt": [TurnInterruptParams, TurnInterruptResponse];
};
