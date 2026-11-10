// ⌘K's search of every workspace (docs/adr/local-hybrid-search.md): its filters as the daemon
// takes them, which workspaces listed by name they keep, what the footer says of search by
// meaning, and the excerpts' highlights.

import type { SearchFilters, SearchStatus, WorkspaceInfo } from "@chauffeur/protocol";
import { workspaceRepoNames } from "@/lib/format";

/** Turns the daemon's excerpt markers into text and highlights; never interprets HTML. */
export function searchExcerpt(snippet: string): { text: string; highlight: boolean }[] {
  let highlight = false;
  return snippet.split(/([\u0001\u0002])/).flatMap((text) => {
    if (text === "\u0001" || text === "\u0002") {
      highlight = text === "\u0001";
      return [];
    }
    return text ? [{ text, highlight }] : [];
  });
}

export type SearchChoices = {
  scope: "all" | "active" | "archived";
  where: "all" | "titles" | "messages" | "notes";
  /** A repo's name; empty: any. */
  repo: string;
  /** A ticket's identifier; empty: any. */
  ticket: string;
  date: "any" | "week" | "month" | "year";
  pr: "any" | "with" | "merged";
};

export const NO_CHOICES: SearchChoices = { scope: "all", where: "all", repo: "", ticket: "", date: "any", pr: "any" };

const DAYS = { week: 7, month: 30, year: 365 } as const;

export function searchFilters(choices: SearchChoices, now: Date): SearchFilters {
  const filters: SearchFilters = {
    sources: choices.where === "all" ? [] : [choices.where],
    repos: choices.repo ? [choices.repo] : [],
    tickets: choices.ticket ? [choices.ticket] : [],
  };
  if (choices.scope !== "all") filters.state = choices.scope;
  if (choices.date !== "any") filters.since = new Date(now.getTime() - DAYS[choices.date] * 86_400_000).toISOString();
  if (choices.pr !== "any") filters.pr = choices.pr === "with" ? "any" : "merged";
  return filters;
}

/**
 * Whether a workspace matched by name stays listed under the choices. A date or a pull request
 * cannot be told from the workspace alone: the daemon's results, which know them, list it then.
 */
export function workspaceKept(workspace: WorkspaceInfo, choices: SearchChoices): boolean {
  const archived = workspace.state === "archived" || workspace.state === "archiving";
  if (choices.scope !== "all" && archived !== (choices.scope === "archived")) return false;
  if (choices.where !== "all" && choices.where !== "titles") return false;
  if (choices.date !== "any" || choices.pr !== "any") return false;
  if (choices.repo && !workspaceRepoNames(workspace).includes(choices.repo)) return false;
  const ticket = choices.ticket.toUpperCase();
  return !ticket || workspace.tickets.some((t) => t.identifier.toUpperCase() === ticket);
}

/** The repos and tickets the filters offer: the workspaces', sorted. */
export function filterOptions(workspaces: WorkspaceInfo[]): { repos: string[]; tickets: string[] } {
  const repos = new Set<string>();
  const tickets = new Set<string>();
  for (const workspace of workspaces) {
    for (const repo of workspaceRepoNames(workspace)) repos.add(repo);
    for (const ticket of workspace.tickets) tickets.add(ticket.identifier);
  }
  const sorted = (values: Set<string>) => [...values].sort((a, b) => a.localeCompare(b, undefined, { numeric: true }));
  return { repos: sorted(repos), tickets: sorted(tickets) };
}

/** The download of the embedding models we measured. */
const MODEL_SIZES: Record<string, string> = { embeddinggemma: "621 MB", "qwen3-embedding:0.6b": "639 MB" };

/**
 * What the footer says of search by meaning, and what it offers to do; none when it works and
 * the whole history has its meaning.
 */
export function semanticLine(status: SearchStatus): { text: string; action: "download" | "settings" | null } | null {
  const size = MODEL_SIZES[status.model];
  switch (status.semantic) {
    case "disabled":
      return { text: "Keywords only: search by meaning is off.", action: "settings" };
    case "no_ollama":
      return { text: "Keywords only: search by meaning needs Ollama, installed from Settings › Local models.", action: "settings" };
    case "no_model":
      return { text: `Keywords only: search by meaning needs ${status.model}${size ? ` (${size})` : ""}.`, action: "download" };
    case "starting":
      return { text: "Keywords only for now: search by meaning is starting…", action: null };
    case "on_battery":
      return { text: `Search by meaning covers ${indexed(status)} of the history; the rest is indexed on the charger.`, action: null };
    case "ready":
      return status.embedded < status.total ? { text: `Indexing by meaning: ${indexed(status)} done.`, action: null } : null;
  }
}

function indexed(status: SearchStatus): string {
  return `${status.total === 0 ? 100 : Math.floor((status.embedded * 100) / status.total)}%`;
}
