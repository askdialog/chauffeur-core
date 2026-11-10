// Claude's canUseTool callback, turned into `permission_request` / `question` events answered by
// `session.respond`.

import type { CanUseTool, PermissionResult } from "@anthropic-ai/claude-agent-sdk";
import type { JsonValue, Question, RequestAgent } from "../../protocol.ts";
import type { Outcome, PendingRequests } from "../pending.ts";
import { DELEGATE_TOOL } from "../delegate-tool.ts";
import { ADD_REPO_TOOL, LIST_REPOS_TOOL, UPDATE_PLAN_TOOL } from "../repo-tools.ts";
import { type AlwaysAllow, AlwaysAllowed, alwaysAllow, ruleCovers, ruleText } from "./always.ts";

export const ASK_USER_QUESTION = "AskUserQuestion";

/**
 * The SDK's permission callback, answered through `requests`. `refuse` denies a call without
 * asking (an exploration's plan guard, again here should the hook not have run). `agent` names
 * the subagent behind a request, from the agent id Claude Code gives when one asks. `asking` is
 * told of the tool call about to be put to the user: its automatic check, if any, is over.
 * `always` holds the rules the user allowed always in this chat: a call they cover is allowed
 * without asking, and the open requests a new one covers are allowed with it.
 */
export function claudeCanUseTool(
  requests: PendingRequests,
  refuse?: (toolName: string, input: Record<string, unknown>) => string | undefined,
  agent?: (agentId: string) => RequestAgent,
  asking?: (toolUseId: string) => void,
  always: AlwaysAllowed = new AlwaysAllowed(),
): CanUseTool {
  return async (toolName, input, options) => {
    const refusal = refuse?.(toolName, input);
    if (refusal !== undefined) return { behavior: "deny", message: refusal };
    asking?.(options.toolUseID);
    // Chauffeur's tools: `add_repo` puts the repo to the user itself (repo-tools.ts), and a
    // delegated chat asks for what its own agent does (delegate-tool.ts).
    if (toolName === LIST_REPOS_TOOL || toolName === ADD_REPO_TOOL || toolName === DELEGATE_TOOL || toolName === UPDATE_PLAN_TOOL) {
      return { behavior: "allow", updatedInput: input };
    }
    // A background subagent's request outlives an interrupt, which ends the turn only (Claude
    // Code withdraws the turn's own requests). Asked then: a subagent may have been moved to the
    // background since it asked.
    const agentId = options.agentID;
    const outlivesTurn = agentId && agent ? () => agent(agentId).background : undefined;
    if (toolName === ASK_USER_QUESTION) {
      const outcome = await requests.ask(
        "question",
        (requestId) => ({ type: "question", request_id: requestId, questions: questionsOf(input) }),
        { signal: options.signal, outlivesTurn },
      );
      return permissionResult(outcome, input, undefined);
    }
    // Claude Code asks again in plan mode whatever the session's rules say: the chat's own answer.
    if (always.covering(toolName, input)) return { behavior: "allow", updatedInput: input };
    // Claude Code says when a lasting rule would grant more than this call: allowed once then.
    const remember = options.suppressAlwaysAllowRule ? undefined : alwaysAllow(toolName, input, options.suggestions);
    const outcome = await requests.ask(
      "permission",
      (requestId) => ({
        type: "permission_request",
        request_id: requestId,
        tool_name: toolName,
        input: input as JsonValue,
        title: options.title ?? null,
        ...(agentId && agent ? { agent: agent(agentId) } : {}),
      }),
      {
        signal: options.signal,
        outlivesTurn,
        subject: { toolName, input },
        resolution: (o) => (remember && o.type === "allow" && o.always ? { rules: remember.rules.map(ruleText) } : {}),
      },
    );
    if (remember && outcome.type === "allow" && outcome.always) {
      always.add(remember.rules);
      requests.allowCovered((call) => {
        const rule = remember.rules.find((r) => ruleCovers(r, call.toolName, call.input));
        return rule && [ruleText(rule)];
      });
    }
    return permissionResult(outcome, input, remember);
  };
}

/** The SDK's answer to a request. Always allow hands Claude Code the rules it remembers. */
export function permissionResult(outcome: Outcome, input: Record<string, unknown>, remember: AlwaysAllow | undefined): PermissionResult {
  switch (outcome.type) {
    case "allow":
      return outcome.always && remember
        ? { behavior: "allow", updatedInput: input, updatedPermissions: remember.updatedPermissions }
        : { behavior: "allow", updatedInput: input };
    case "answer":
      return { behavior: "allow", updatedInput: { ...input, answers: outcome.answers } };
    case "deny":
      return { behavior: "deny", message: outcome.message ?? "Denied by the user" };
    case "elicitation":
      return { behavior: "deny", message: "MCP confirmations must use their own request" };
    case "cancelled":
      // Read only when Chauffeur cancels it on an interrupt: Claude Code ignores the answer to a
      // request it withdrew itself, and is gone after a stop.
      return { behavior: "deny", message: "The user interrupted this request" };
  }
}

/** AskUserQuestion's input, defensively mapped onto the protocol's Question. */
export function questionsOf(input: Record<string, unknown>): Question[] {
  const questions = Array.isArray(input.questions) ? input.questions : [];
  return questions.map((raw: unknown) => {
    const q = (typeof raw === "object" && raw !== null ? raw : {}) as Record<string, unknown>;
    const options = Array.isArray(q.options) ? q.options : [];
    return {
      question: typeof q.question === "string" ? q.question : "",
      header: typeof q.header === "string" ? q.header : "",
      options: options.map((rawOption: unknown) => {
        const o = (typeof rawOption === "object" && rawOption !== null ? rawOption : {}) as Record<string, unknown>;
        return {
          label: typeof o.label === "string" ? o.label : "",
          description: typeof o.description === "string" ? o.description : "",
          // Markdown (the session asks for it): shown beside the options while this one is focused.
          ...(typeof o.preview === "string" && o.preview.trim() ? { preview: o.preview } : {}),
        };
      }),
      multi_select: q.multiSelect === true,
    };
  });
}
