// Chauffeur's tools for a Vibe session: `list_repos`, `add_repo` and, but in a delegated chat,
// `delegate`, served by the sidecar's MCP server (acp/tools-server.ts). Vibe cancels a tool a
// program of its code mode calls after 60 s, whatever the server's timeout, so the tools that wait
// (for the user's answer, for a delegated chat) answer within ANSWER_WITHIN_MS: `delegate` leaves
// the delegated chat to work on, its answer coming as a new message (as when a turn ends before it
// answered), and `add_repo` says the user has not answered yet, their answer coming as a message
// the session sends (`followUp`).

import { DELEGATE, DELEGATE_DESCRIPTION, DELEGATE_SCHEMA, delegate, type DelegateHost } from "../delegate-tool.ts";
import type { PendingRequests } from "../pending.ts";
import {
  ADD_REPO,
  ADD_REPO_DESCRIPTION,
  ADD_REPO_SCHEMA,
  addRepo,
  LIST_REPOS,
  LIST_REPOS_DESCRIPTION,
  LIST_REPOS_SCHEMA,
  listRepos,
  type RepoToolsHost,
  type ToolText,
} from "../repo-tools.ts";
import type { ServedTool } from "../acp/tools-server.ts";

export const ANSWER_WITHIN_MS = 45_000;

export interface VibeToolsOptions {
  delegate: boolean;
  /** Aborted by the user's interrupt or the session's stop: a delegated chat is interrupted too. */
  interrupted: () => AbortSignal;
  /** Sends the agent a message of Chauffeur's, for an answer that came after its tool call. */
  followUp: (text: string) => void;
  answerWithinMs?: number;
}

export function vibeChauffeurTools(host: RepoToolsHost & DelegateHost, requests: PendingRequests, options: VibeToolsOptions): ServedTool[] {
  const within = options.answerWithinMs ?? ANSWER_WITHIN_MS;
  const tools: ServedTool[] = [
    { name: LIST_REPOS, description: LIST_REPOS_DESCRIPTION, inputSchema: LIST_REPOS_SCHEMA, run: () => listRepos(host) },
    {
      name: ADD_REPO,
      description: ADD_REPO_DESCRIPTION,
      inputSchema: ADD_REPO_SCHEMA,
      run: async (input) => {
        const asking = addRepo(host, requests, input, { signal: options.interrupted() });
        const answered = await answerWithin(asking, within);
        if (answered) return answered;
        void asking.then((said) => options.followUp(`Chauffeur: the user answered your add_repo call. ${said.text}`));
        return {
          text:
            "The user has not answered yet. Their answer will come to you as a new message: go on with what does not " +
            "need the repo, and do not call add_repo again for it meanwhile.",
          error: false,
        };
      },
    },
  ];
  if (options.delegate) {
    tools.push({
      name: DELEGATE,
      description: DELEGATE_DESCRIPTION,
      inputSchema: DELEGATE_SCHEMA,
      run: async (input) => {
        const detach = new AbortController();
        const timer = setTimeout(() => detach.abort(), within);
        try {
          return await delegate(host, input, { signal: options.interrupted(), detach: detach.signal });
        } finally {
          clearTimeout(timer);
        }
      },
    });
  }
  return tools;
}

async function answerWithin(promise: Promise<ToolText>, ms: number): Promise<ToolText | undefined> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([promise, new Promise<undefined>((resolve) => (timer = setTimeout(() => resolve(undefined), ms)))]);
  } finally {
    clearTimeout(timer);
  }
}
