// Codex's side of Chauffeur's tools: the repo tools (story 48) and, but in a delegated chat,
// `delegate` (story 49), dynamic tools in the `chauffeur` namespace given at `thread/start`
// (Codex's experimental API) and kept by the thread when resumed. Codex calls them with
// `item/tool/call`, answered once the tool is done (for `add_repo`, once the user answered; for
// `delegate`, once the delegated chat did).

import { DELEGATE, DELEGATE_DESCRIPTION, DELEGATE_SCHEMA, delegate, type DelegateHost } from "../delegate-tool.ts";
import type { PendingRequests } from "../pending.ts";
import {
  type AddRepoOptions,
  ADD_REPO,
  ADD_REPO_DESCRIPTION,
  ADD_REPO_SCHEMA,
  addRepo,
  CHAUFFEUR_TOOLS,
  LIST_REPOS,
  LIST_REPOS_DESCRIPTION,
  LIST_REPOS_SCHEMA,
  listRepos,
  type RepoToolsHost,
  type ToolText,
  TOOLS_DESCRIPTION,
} from "../repo-tools.ts";
import type { DynamicToolCallParams, DynamicToolCallResponse, DynamicToolSpec } from "./protocol.ts";

/** The tools a thread starts with: `delegate` but in a delegated chat. */
export function codexChauffeurTools(options: { delegate?: boolean } = {}): DynamicToolSpec[] {
  return [
    {
      type: "namespace",
      name: CHAUFFEUR_TOOLS,
      description: TOOLS_DESCRIPTION,
      tools: [
        { type: "function", name: LIST_REPOS, description: LIST_REPOS_DESCRIPTION, inputSchema: LIST_REPOS_SCHEMA },
        { type: "function", name: ADD_REPO, description: ADD_REPO_DESCRIPTION, inputSchema: ADD_REPO_SCHEMA },
        ...(options.delegate
          ? [{ type: "function" as const, name: DELEGATE, description: DELEGATE_DESCRIPTION, inputSchema: DELEGATE_SCHEMA }]
          : []),
      ],
    },
  ];
}

/**
 * Runs a dynamic tool call; `options` as `addRepo`'s (Codex withdrew the call, the request it
 * asked), the signal also interrupting a delegated chat, and `detach` leaving it to work on.
 */
export async function codexToolCall(
  params: DynamicToolCallParams,
  host: RepoToolsHost & DelegateHost,
  requests: PendingRequests,
  options: AddRepoOptions & { detach?: AbortSignal } = {},
): Promise<DynamicToolCallResponse> {
  let answer: ToolText;
  if (params.namespace !== CHAUFFEUR_TOOLS) answer = { text: `Chauffeur has no tool ${params.tool}.`, error: true };
  else if (params.tool === LIST_REPOS) answer = await listRepos(host);
  else if (params.tool === ADD_REPO) answer = await addRepo(host, requests, params.arguments, options);
  else if (params.tool === DELEGATE) {
    answer = await delegate(host, params.arguments, { signal: options.signal, detach: options.detach, toolUseId: params.callId });
  } else answer = { text: `Chauffeur has no tool ${params.tool}.`, error: true };
  return { contentItems: [{ type: "inputText", text: answer.text }], success: !answer.error };
}

/** One of Chauffeur's tool calls once Codex has its result. */
export interface FinishedToolCall {
  turnId: string;
  startedAt: number;
  endedAt: number;
}

/** A tool call that ran this long holds back the calls Codex made beside it. */
const HELD_BACK_AFTER_MS = 30_000;
/** Codex's model takes longer than this to answer a result with a new call. */
const NEXT_CALL_MS = 1_000;

/**
 * Codex runs the calls of one response to Chauffeur's tools one after another: a second `delegate`
 * beside the first only starts when the first has answered, maybe long after its model has moved
 * on and given that work to another chat. A `delegate` call
 * that reaches Chauffeur the instant a long call of its turn ended is such a call: it is refused,
 * with what happened, and the model calls again if the work is still to give.
 */
export function heldBackDelegate(params: DynamicToolCallParams, previous: FinishedToolCall | undefined, now: number): ToolText | null {
  if (params.namespace !== CHAUFFEUR_TOOLS || params.tool !== DELEGATE || !previous) return null;
  if (previous.turnId !== params.turnId || now - previous.endedAt > NEXT_CALL_MS) return null;
  const held = previous.endedAt - previous.startedAt;
  if (held < HELD_BACK_AFTER_MS) return null;
  return {
    text:
      `Chauffeur started no delegated chat for this call: Codex held it back until the call made beside it had ` +
      `answered, about ${Math.round(held / 60_000)} min after you made it. If its task was not given to another chat ` +
      `since, call delegate again; to run several delegated chats at once, make each call in its own response.`,
    error: true,
  };
}

