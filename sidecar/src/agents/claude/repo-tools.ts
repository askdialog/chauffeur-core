// Claude's side of Chauffeur's tools: an in-process MCP server, `chauffeur`, whose tools are always
// loaded (never behind tool search) and run in the sidecar: the repo tools (story 48), `update_plan`
// (the agent's plan, counted into the chat's progress) and, but in a delegated chat, `delegate`
// (story 49). The chat allows them itself (options.ts, requests.ts):
// `add_repo` asks the user on its own, whatever the permission mode.

import { createSdkMcpServer, type McpSdkServerConfigWithInstance, tool } from "@anthropic-ai/claude-agent-sdk";
import { z } from "zod";
import { SIDECAR_VERSION } from "../../version.ts";
import { DELEGATE, DELEGATE_AGENTS, DELEGATE_DESCRIPTION, DELEGATE_INPUT, DELEGATE_TIERS, delegate, type DelegateHost } from "../delegate-tool.ts";
import type { PendingRequests } from "../pending.ts";
import { PLAN_STATUSES, recordPlan, UPDATE_PLAN, UPDATE_PLAN_DESCRIPTION, UPDATE_PLAN_STEPS } from "../plan-tool.ts";
import {
  ADD_REPO,
  ADD_REPO_DESCRIPTION,
  addRepo,
  CHAUFFEUR_TOOLS,
  LIST_REPOS,
  LIST_REPOS_DESCRIPTION,
  listRepos,
  type RepoToolsHost,
  type ToolText,
  TOOLS_DESCRIPTION,
} from "../repo-tools.ts";

/**
 * How long a call may wait: `add_repo` waits for the user, who may answer much later, and
 * `delegate` for the delegated chat (Claude Code otherwise fails a tool call after its own MCP timeout).
 */
export const REPO_TOOLS_TIMEOUT_MS = 7 * 24 * 60 * 60 * 1000;

export function claudeRepoTools(
  host: RepoToolsHost & DelegateHost,
  requests: PendingRequests,
  options: { delegate?: boolean } = {},
): McpSdkServerConfigWithInstance {
  const delegateTool = tool(
    DELEGATE,
    DELEGATE_DESCRIPTION,
    {
      prompt: z.string().describe(DELEGATE_INPUT.prompt),
      review: z.boolean().optional().describe(DELEGATE_INPUT.review),
      tier: z.enum(DELEGATE_TIERS).optional().describe(DELEGATE_INPUT.tier),
      effort: z.string().optional().describe(DELEGATE_INPUT.effort),
      agent: z.enum(DELEGATE_AGENTS).optional().describe(DELEGATE_INPUT.agent),
      model: z.string().optional().describe(DELEGATE_INPUT.model),
      chat: z.string().optional().describe(DELEGATE_INPUT.chat),
    },
    async (args, extra) => callResult(await delegate(host, args, { signal: signalOf(extra), toolUseId: toolUseIdOf(extra) })),
    { annotations: { readOnlyHint: false, destructiveHint: false, openWorldHint: false } },
  );
  return createSdkMcpServer({
    name: CHAUFFEUR_TOOLS,
    version: SIDECAR_VERSION,
    instructions: TOOLS_DESCRIPTION,
    alwaysLoad: true,
    timeout: REPO_TOOLS_TIMEOUT_MS,
    tools: [
      tool(LIST_REPOS, LIST_REPOS_DESCRIPTION, {}, async () => callResult(await listRepos(host)), {
        annotations: { readOnlyHint: true, openWorldHint: false },
      }),
      tool(
        ADD_REPO,
        ADD_REPO_DESCRIPTION,
        {
          repo: z.string().describe("The repo's name, as list_repos gives it (e.g. analytics)."),
          reason: z.string().describe("Why the work needs this repo, in one sentence, in the user's language."),
        },
        async (args, extra) => callResult(await addRepo(host, requests, args, { signal: signalOf(extra) })),
        { annotations: { readOnlyHint: false, destructiveHint: false, openWorldHint: false } },
      ),
      ...(options.delegate ? [delegateTool] : []),
      tool(
        UPDATE_PLAN,
        UPDATE_PLAN_DESCRIPTION,
        {
          steps: z
            .array(z.object({ text: z.string(), status: z.enum(PLAN_STATUSES) }))
            .describe(UPDATE_PLAN_STEPS),
        },
        async (args) => callResult(recordPlan(args)),
        { annotations: { readOnlyHint: true, openWorldHint: false } },
      ),
    ],
  });
}

function callResult(answer: ToolText) {
  return { content: [{ type: "text" as const, text: answer.text }], ...(answer.error ? { isError: true } : {}) };
}

/** Claude Code's id for the call (`tool_use_id`), which it passes in the MCP request's `_meta`. */
function toolUseIdOf(extra: unknown): string | undefined {
  const meta = (extra as { _meta?: unknown } | null)?._meta;
  const id = (meta as Record<string, unknown> | null | undefined)?.["claudecode/toolUseId"];
  return typeof id === "string" && id ? id : undefined;
}

/** The MCP request's abort signal: Claude Code gave up on the call (an interrupt). */
function signalOf(extra: unknown): AbortSignal | undefined {
  const signal = (extra as { signal?: unknown } | null)?.signal;
  return signal instanceof AbortSignal ? signal : undefined;
}
