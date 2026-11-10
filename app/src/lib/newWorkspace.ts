// The creation dialog's logic (⌘N): the branch it will create, the repos a ticket brought, the
// notice that a ticket starts, and the field a refusal names. Mirrors the daemon's rules
// (`crates/chauffeurd/src/slug.rs`, `Workspaces::create`) so the dialog shows them before creating.

import type { LinearIssue, RepoInfo } from "@chauffeur/protocol";
import { RpcCallError } from "./rpc";

export type CreateKind = "workspace" | "explore" | "design";

/** What a linked ticket brought, kept beside its `TicketRef`. */
export type LinkedTicket = Pick<LinearIssue, "identifier" | "branch_name" | "state" | "state_type" | "repos">;

const ACCENTS: Record<string, string> = {
  à: "a", â: "a", ä: "a", á: "a",
  é: "e", è: "e", ê: "e", ë: "e",
  î: "i", ï: "i", í: "i",
  ô: "o", ö: "o", ó: "o",
  ù: "u", û: "u", ü: "u", ú: "u",
  ç: "c",
};

/** The daemon's `slugify`: ASCII letters and digits, lower-cased, runs of others as one `-`. */
export function slugify(text: string, max: number): string {
  let out = "";
  let dash = false;
  for (const raw of text) {
    const c = ACCENTS[raw] ?? raw;
    if (/^[A-Za-z0-9]$/.test(c)) {
      if (dash && out) out += "-";
      out += c.toLowerCase();
      dash = false;
    } else {
      dash = true;
    }
  }
  if (out.length > max) out = out.slice(0, max).replace(/-+$/, "");
  return out;
}

/** The branches' prefix: the setting, else the GitHub login, else `chauffeur`. */
export function branchPrefix(settings: { branch_prefix: string; github_login: string | null } | null): string {
  return settings?.branch_prefix || settings?.github_login || "chauffeur";
}

export type BranchPreview =
  /** No branch: an exploration's or a design's checkouts. */
  | { kind: "none" }
  /** Known in full; `from` says where it comes from. */
  | { kind: "known"; branch: string; from: string | null }
  /** The prefix is known, the rest is named later. */
  | { kind: "later"; prefix: string; note: string };

/**
 * The branch `workspace.create` will make, in the daemon's order: typed, a child's
 * `<ID>/<title>`, one ticket's Linear branch, the name typed, one ticket's id and title, an
 * imported share's title, else named from the first prompt.
 */
export function branchPreview({
  kind,
  prefix,
  branch,
  name,
  child,
  tickets,
  linked,
  shareTitle,
}: {
  kind: CreateKind;
  prefix: string;
  branch: string;
  name: string;
  child: boolean;
  /** The picked tickets: identifiers and titles. */
  tickets: Array<{ identifier: string; title: string | null }>;
  linked: Record<string, LinkedTicket>;
  shareTitle: string | null;
}): BranchPreview {
  if (kind !== "workspace") return { kind: "none" };
  if (branch.trim()) return { kind: "known", branch: branch.trim(), from: null };
  const ticket = tickets.length === 1 ? tickets[0] : null;
  if (child && ticket) {
    return {
      kind: "known",
      branch: ticket.title ? `${ticket.identifier}/${slugify(ticket.title, 50)}` : ticket.identifier,
      from: `from ${ticket.identifier}`,
    };
  }
  const linearBranch = ticket ? linked[ticket.identifier]?.branch_name : null;
  if (ticket && linearBranch) return { kind: "known", branch: linearBranch, from: `from ${ticket.identifier} in Linear` };
  if (name.trim()) return { kind: "known", branch: `${prefix}/${slugify(name, 60)}`, from: null };
  if (ticket) {
    const id = slugify(ticket.identifier, 20);
    const title = ticket.title ? slugify(ticket.title, 40) : "";
    return { kind: "known", branch: `${prefix}/${title ? `${id}-${title}` : id}`, from: `from ${ticket.identifier}` };
  }
  if (shareTitle) return { kind: "known", branch: `${prefix}/${slugify(`${shareTitle} (fork)`, 60)}`, from: null };
  return { kind: "later", prefix: `${prefix}/`, note: "named from the first prompt" };
}

/** The repos chosen, and which ticket brought each one a ticket added. */
export type RepoChoice = { selection: string[]; sources: Record<string, string> };

/** A ticket linked: the repos its `## Repo` section names join the choice, marked as its own. */
export function withTicketRepos(choice: RepoChoice, ticket: string, repos: string[]): RepoChoice {
  const added = repos.filter((repo) => !choice.selection.includes(repo));
  if (added.length === 0) return choice;
  const sources = { ...choice.sources };
  for (const repo of added) sources[repo] = ticket;
  return { selection: [...choice.selection, ...added], sources };
}

/** A ticket unlinked: the repos it added go with it. */
export function withoutTicketRepos(choice: RepoChoice, ticket: string): RepoChoice {
  const brought = Object.keys(choice.sources).filter((repo) => choice.sources[repo] === ticket);
  if (brought.length === 0) return choice;
  const sources = { ...choice.sources };
  for (const repo of brought) delete sources[repo];
  return { selection: choice.selection.filter((repo) => !brought.includes(repo)), sources };
}

/** A repo toggled by hand: it is the user's own from then on. */
export function toggledRepo(choice: RepoChoice, repo: string): RepoChoice {
  const sources = { ...choice.sources };
  delete sources[repo];
  return choice.selection.includes(repo)
    ? { selection: choice.selection.filter((r) => r !== repo), sources }
    : { selection: [...choice.selection, repo], sources };
}

/** Whether creating moves `ticket` to In Progress: the setting is on, the work changes code,
 *  and the ticket has not started (nor ended). */
export function startsTicket(ticket: LinkedTicket | undefined, startTickets: boolean, kind: CreateKind): boolean {
  if (!ticket || !startTickets || kind !== "workspace") return false;
  return !["started", "completed", "canceled"].includes(ticket.state_type ?? "");
}

/** Where an exploration's repos are checked out: their shared base branch, when they share one. */
export function checkoutBranch(repos: RepoInfo[] | null, selection: string[]): string | null {
  const branches = new Set(selection.map((key) => repos?.find((r) => r.key === key)?.default_branch ?? null));
  if (branches.size !== 1) return null;
  const [branch] = branches;
  return branch ? `origin/${branch}` : null;
}

/** A refusal of `workspace.create` that names the branch, with the name it suggests instead. */
export function branchErrorOf(error: unknown): { suggestion: string | null } | null {
  if (!(error instanceof RpcCallError) || typeof error.data !== "object" || error.data === null) return null;
  const data = error.data as { field?: unknown; suggestion?: unknown };
  if (data.field !== "branch") return null;
  return { suggestion: typeof data.suggestion === "string" ? data.suggestion : null };
}

/** The one-line explanation under the kind switch. */
export const KIND_HINTS: Record<CreateKind, string> = {
  workspace: "Its own branch in each repo. Agents change code and open PRs.",
  explore: "Repos at their base branch, ready in seconds. Agents read and explain; nothing changes.",
  design: "Screens drawn by an agent on a canvas. The code stays unchanged.",
};

/** A link to a colleague's shared design (`<service>/s/<id>`), as a take-over reads it. */
export function isDesignShareLink(text: string): boolean {
  return /^https?:\/\/\S*\/s\/[0-9a-f]{8,}(?:[/?#]\S*)?$/i.test(text.trim());
}

/** A link to a colleague's shared workspace (`<service>/w/<id>`). */
export function isWorkspaceShareLink(text: string): boolean {
  return /^https?:\/\/[^/\s]+\/w\/[0-9a-f]{8,64}\/?$/i.test(text.trim());
}

/** A public Claude artifact link, without credentials or a custom port. */
export function isClaudeArtifactLink(text: string): boolean {
  try {
    const url = new URL(text.trim());
    return url.protocol === "https:" && url.hostname === "claude.ai" && !url.port && !url.username && !url.password
      && /^\/artifact\/[A-Za-z0-9-]+\/?$/.test(url.pathname);
  } catch {
    return false;
  }
}
