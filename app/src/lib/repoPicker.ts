// The Repos field of the creation dialog: every GitHub owner's repos, grouped (Selected, Recent, then one
// section per owner, then the clones only found on this Mac), filtered by owner and by a search,
// and a pasted GitHub URL offered as a repo of its own.

import type { RepoInfo } from "@chauffeur/protocol";

/** Repos of the last workspaces shown first, at most. */
export const RECENT_MAX = 4;
/** The owner filter that shows every owner. */
export const ALL_OWNERS = "";
/** GitHub CLI's OAuth app on GitHub: where its access to each organization (SSO included) is
 *  granted. */
export const GITHUB_ACCESS_URL = "https://github.com/settings/connections/applications/178c6fc778ccc68e1d6a";

export type RepoSection = {
  /** `recent`, `owner:<login>` or `local`. */
  id: string;
  title: string;
  /** The owner whose avatar the rows show once (owner sections); none in mixed sections. */
  owner: string | null;
  /** Shown at the right of the title: the owner's repos, or `matches of all` while searching. */
  count: string | null;
  repos: RepoInfo[];
};

/** The main organization: the owner whose repos are named without it. */
export function mainOwner(repos: RepoInfo[]): string | null {
  return repos.find((r) => !r.local && r.owner && r.key === r.name)?.owner ?? null;
}

/** The owners to filter on, with their repo counts: the main organization, the account, then
 *  the others by number of repos. */
export function ownerFilters(repos: RepoInfo[], account: string | null): { owner: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const repo of repos) {
    if (repo.local || !repo.owner) continue;
    counts.set(repo.owner, (counts.get(repo.owner) ?? 0) + 1);
  }
  const main = mainOwner(repos);
  const rank = (owner: string) => (owner === main ? 0 : owner.toLowerCase() === account?.toLowerCase() ? 1 : 2);
  return [...counts.entries()]
    .map(([owner, count]) => ({ owner, count }))
    .sort((a, b) => rank(a.owner) - rank(b.owner) || b.count - a.count || a.owner.localeCompare(b.owner));
}

/** Whether a row in a mixed list (Recent, the chips) says its owner: outside the main
 *  organization, or when another owner has a repo of the same name. */
export function showsOwner(repo: RepoInfo, repos: RepoInfo[]): boolean {
  if (repo.local || !repo.owner) return false;
  return repo.key !== repo.name || repos.some((other) => other !== repo && other.name === repo.name);
}

export function matchesQuery(repo: RepoInfo, query: string): boolean {
  const needle = query.trim().toLowerCase();
  if (!needle) return true;
  return (
    repo.key.toLowerCase().includes(needle) ||
    repo.full_name.toLowerCase().includes(needle) ||
    !!repo.description?.toLowerCase().includes(needle)
  );
}

/** The sections the list shows, in order: the chosen repos first (Selected), then Recent only
 *  without filter nor search; a repo shown in either is not repeated below. */
export function repoSections(
  repos: RepoInfo[],
  {
    owner = ALL_OWNERS,
    query = "",
    account = null,
    selected = [],
  }: { owner?: string; query?: string; account?: string | null; selected?: string[] },
): RepoSection[] {
  const searching = query.trim() !== "";
  const sections: RepoSection[] = [];
  const chosen = selected
    .map((key) => repos.find((r) => r.key === key))
    .filter((r): r is RepoInfo => !!r && (owner === ALL_OWNERS || r.owner === owner) && matchesQuery(r, query));
  if (chosen.length > 0) sections.push({ id: "selected", title: "Selected", owner: null, count: null, repos: chosen });
  const picked = new Set(chosen.map((r) => r.key));
  const recent =
    owner === ALL_OWNERS && !searching
      ? repos
          .filter((r) => r.last_used && !picked.has(r.key))
          .sort((a, b) => (b.last_used ?? "").localeCompare(a.last_used ?? ""))
          .slice(0, RECENT_MAX)
      : [];
  if (recent.length > 0) sections.push({ id: "recent", title: "Recent", owner: null, count: null, repos: recent });
  const shown = new Set([...picked, ...recent.map((r) => r.key)]);
  const count = (all: RepoInfo[], matched: RepoInfo[]) =>
    searching ? `${matched.length} of ${all.length}` : `${all.length}`;
  const owners = ownerFilters(repos, account)
    .map((o) => o.owner)
    .filter((o) => owner === ALL_OWNERS || o === owner);
  for (const login of owners) {
    const all = repos.filter((r) => !r.local && r.owner === login);
    const matched = all.filter((r) => matchesQuery(r, query));
    const rows = matched.filter((r) => !shown.has(r.key));
    if (rows.length > 0) sections.push({ id: `owner:${login}`, title: login, owner: login, count: count(all, matched), repos: rows });
  }
  if (owner === ALL_OWNERS) {
    // Clones no owner lists, and repos named in the settings without an owner.
    const all = repos.filter((r) => r.local || !r.owner);
    const matched = all.filter((r) => matchesQuery(r, query));
    const rows = matched.filter((r) => !shown.has(r.key));
    if (rows.length > 0) sections.push({ id: "local", title: "On this Mac", owner: null, count: count(all, matched), repos: rows });
  }
  return sections;
}

/** `owner/name` from a pasted GitHub URL (`https://github.com/o/n`, `github.com/o/n.git`,
 *  `git@github.com:o/n`) or a typed `owner/name`; null otherwise. */
export function githubRepoRef(text: string): string | null {
  const trimmed = text.trim();
  const url = trimmed.match(/^(?:https?:\/\/)?(?:www\.)?github\.com[/:]([^/\s]+)\/([^/\s#?]+)/i)
    ?? trimmed.match(/^git@github\.com:([^/\s]+)\/([^/\s#?]+)/i)
    ?? trimmed.match(/^([^/\s:]+)\/([^/\s#?]+)$/);
  if (!url) return null;
  const owner = url[1];
  const name = url[2].replace(/\.git$/i, "");
  if (!/^[A-Za-z0-9](?:[A-Za-z0-9-]*)$/.test(owner) || !/^[A-Za-z0-9_.-]+$/.test(name) || name.startsWith(".")) return null;
  return `${owner}/${name}`;
}

/** The last push, short: `now`, `5m`, `2h`, `3d`, `3w`, `2mo`, `1y`. */
export function shortAge(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const minutes = Math.max(0, Math.floor((now - t) / 60_000));
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d`;
  if (days < 30) return `${Math.floor(days / 7)}w`;
  if (days < 365) return `${Math.floor(days / 30)}mo`;
  return `${Math.floor(days / 365)}y`;
}

const AVATAR_COLORS = ["#5b5fd6", "#b8693f", "#2d8a78", "#8a5096", "#3f7fb8", "#a0533f", "#4f8a3f", "#9a7b2d"];

/** An owner's placeholder avatar: its initials on a color of its own. */
export function ownerAvatar(owner: string): { initials: string; color: string } {
  const parts = owner.split(/[-_.]+/).filter(Boolean);
  const initials = (parts.length >= 2 ? parts[0][0] + parts[1][0] : owner.slice(0, 2)).toUpperCase();
  let hash = 0;
  for (const char of owner.toLowerCase()) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return { initials, color: AVATAR_COLORS[hash % AVATAR_COLORS.length] };
}
