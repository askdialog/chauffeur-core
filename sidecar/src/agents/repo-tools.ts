// The agent's tools for its workspace's repos (story 48), the same for every agent: `list_repos`
// lists the repos the user can pick, `add_repo` proposes one to the user and, once they said yes,
// asks the daemon to add it. The user is asked here, by the tool itself, so that no permission
// mode, rule or Always allow can skip the question. Claude gets the tools from an in-process MCP
// server, Codex as dynamic tools; both show as `mcp__chauffeur__*`.

import type { JsonValue, ToolAddRepoResult, ToolListReposResult } from "../protocol.ts";
import { errorMessage } from "../log.ts";
import type { Outcome, PendingRequests } from "./pending.ts";

/** The namespace of Chauffeur's own tools: Claude's MCP server, Codex's dynamic tool namespace. */
export const CHAUFFEUR_TOOLS = "chauffeur";
export const LIST_REPOS = "list_repos";
export const ADD_REPO = "add_repo";
/** The tools' names as the chat shows them, whatever the agent. */
export const LIST_REPOS_TOOL = `mcp__${CHAUFFEUR_TOOLS}__${LIST_REPOS}`;
export const ADD_REPO_TOOL = `mcp__${CHAUFFEUR_TOOLS}__${ADD_REPO}`;
/** Claude's plan tool (plan-tool.ts), as the chat shows it. */
export const UPDATE_PLAN_TOOL = `mcp__${CHAUFFEUR_TOOLS}__update_plan`;

export const TOOLS_DESCRIPTION = "Chauffeur's tools for this workspace: its repos, and delegated chats of other agents.";

export const LIST_REPOS_DESCRIPTION =
  "List the repos that this Chauffeur workspace can include: each one's name (`owner/name` outside the main " +
  "organization) and description, and the folder of those already in the workspace. Call it before add_repo when you " +
  "are unsure of a repo's name.";

export const ADD_REPO_DESCRIPTION =
  "Propose adding a repo to this Chauffeur workspace, when the work needs a repo the workspace lacks (code to change " +
  "or read there). The user is asked and answers Add or Not now; the call returns once they did. Added, the repo is " +
  "checked out in the workspace folder on the workspace's branch, and its setup script runs. Say in `reason`, in one " +
  "sentence in the user's language, why the work needs it. Never clone a repo yourself.";

/** `add_repo`'s input as a JSON schema (Codex's dynamic tools). */
export const ADD_REPO_SCHEMA: JsonValue = {
  type: "object",
  properties: {
    repo: { type: "string", description: "The repo's name, as list_repos gives it (e.g. analytics)." },
    reason: { type: "string", description: "Why the work needs this repo, in one sentence, in the user's language." },
  },
  required: ["repo", "reason"],
  additionalProperties: false,
};

export const LIST_REPOS_SCHEMA: JsonValue = { type: "object", properties: {}, additionalProperties: false };

/** What the tools need from the daemon, scoped to the chat. */
export interface RepoToolsHost {
  listRepos(): Promise<ToolListReposResult>;
  addRepo(repo: string): Promise<ToolAddRepoResult>;
}

/** A tool's answer to the agent. */
export interface ToolText {
  text: string;
  error: boolean;
}

export async function listRepos(host: RepoToolsHost): Promise<ToolText> {
  let listed: ToolListReposResult;
  try {
    listed = await host.listRepos();
  } catch (error) {
    return { text: `Chauffeur could not list the repos: ${errorMessage(error)}`, error: true };
  }
  const line = (repo: ToolListReposResult["repos"][number]) =>
    `- ${repo.name}${repo.description ? `: ${repo.description}` : ""}`;
  const inside = listed.repos.filter((repo) => repo.path);
  const outside = listed.repos.filter((repo) => !repo.path);
  const parts = [
    `In this ${listed.explore ? "exploration" : "workspace"} (${listed.workspace}):`,
    ...inside.map((repo) => `${line(repo)} (folder ${repo.path})`),
  ];
  parts.push("", outside.length > 0 ? "Not in it, can be proposed with add_repo:" : "Every repo of the organization is in it.");
  parts.push(...outside.map(line));
  return { text: parts.join("\n"), error: false };
}

export interface AddRepoOptions {
  /** The agent gave up on the call: the question is withdrawn. */
  signal?: AbortSignal;
  /** Told the id of the request put to the user (Codex maps its own request onto it). */
  asked?: (requestId: string) => void;
}

/**
 * `add_repo`: checks the repo, puts it to the user (a `permission_request` for ADD_REPO_TOOL,
 * titled with the repo, which the app draws as its own card), then asks the daemon to add it.
 */
export async function addRepo(
  host: RepoToolsHost,
  requests: PendingRequests,
  input: unknown,
  options: AddRepoOptions = {},
): Promise<ToolText> {
  const fields = (typeof input === "object" && input !== null ? input : {}) as Record<string, unknown>;
  const asked = typeof fields.repo === "string" ? fields.repo.trim() : "";
  const reason = typeof fields.reason === "string" ? fields.reason.trim() : "";
  if (!asked) return { text: "Name the repo to add in `repo` (list_repos gives the names).", error: true };
  let listed: ToolListReposResult;
  try {
    listed = await host.listRepos();
  } catch (error) {
    return { text: `Chauffeur could not list the repos: ${errorMessage(error)}`, error: true };
  }
  // Repos outside the main organization are listed as `owner/name`; `acme/analytics`
  // names the main organization's `analytics`, and case does not matter.
  const lower = asked.toLowerCase();
  const short = (name: string) => name.slice(name.lastIndexOf("/") + 1).toLowerCase();
  const byShort = listed.repos.filter((r) => short(r.name) === short(asked));
  const repo =
    listed.repos.find((r) => r.name === asked) ??
    listed.repos.find((r) => r.name.toLowerCase() === lower) ??
    (asked.includes("/") ? byShort.find((r) => !r.name.includes("/")) : byShort.length === 1 ? byShort[0] : undefined);
  if (!repo) {
    return { text: `${asked} is not one of the repos you can add. Call list_repos for their names.`, error: true };
  }
  if (repo.path) return { text: `${repo.name} is already in this workspace, in ${repo.path}.`, error: false };
  const where = listed.explore ? "exploration" : "workspace";
  if (listed.state !== "ready") {
    return { text: `This ${where} is ${listed.state}: no repo can be added to it now.`, error: true };
  }
  // Given up while the repos were read: nothing is asked.
  if (options.signal?.aborted) return withdrawn(repo.name);
  const proposal: Record<string, JsonValue> = { repo: repo.name, reason, description: repo.description };
  const outcome: Outcome = await requests.ask(
    "permission",
    (requestId) => {
      options.asked?.(requestId);
      return {
        type: "permission_request",
        request_id: requestId,
        tool_name: ADD_REPO_TOOL,
        input: proposal,
        title: `Add ${repo.name} to this ${where}?`,
      };
    },
    { signal: options.signal },
  );
  switch (outcome.type) {
    case "cancelled":
      return withdrawn(repo.name);
    case "deny":
    case "answer":
      return {
        text:
          `The user chose not to add ${repo.name} now${outcome.type === "deny" && outcome.message ? `: ${outcome.message}` : "."} ` +
          "Go on without it, and do not propose it again unless the user brings it up.",
        error: false,
      };
    case "allow":
      break;
  }
  let added: ToolAddRepoResult;
  try {
    added = await host.addRepo(repo.name);
  } catch (error) {
    return { text: `The user said yes, but Chauffeur could not add ${repo.name}: ${errorMessage(error)}`, error: true };
  }
  const how = added.branch ? `on the workspace's branch ${added.branch}` : "as a detached checkout of its base branch";
  return {
    text: [
      `The user said yes: ${repo.name} is in the ${where}, in ${added.path}, ${how}.`,
      added.setup ? "Its setup script runs in the background (the user sees it in Chauffeur's Run tab)." : undefined,
      "This session has not loaded its instructions, hooks or skills (they load from the user's next message): " +
        "read its CLAUDE.md or AGENTS.md before working in it.",
    ]
      .filter(Boolean)
      .join(" "),
    error: false,
  };
}

function withdrawn(repo: string): ToolText {
  return { text: `The proposal was withdrawn before the user answered: ${repo} was not added.`, error: true };
}
