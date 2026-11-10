import type { WorkspaceInfo } from "@chauffeur/protocol";

/** Decodes base64 to raw bytes (terminal output is bytes, not necessarily UTF-8 boundaries). */
export function base64ToBytes(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function formatDuration(ms: number | null | undefined): string {
  if (ms == null) return "";
  if (ms < 1000) return `${ms} ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)} s`;
  const m = Math.floor(s / 60);
  const rest = Math.round(s % 60);
  if (m < 60) return `${m}m ${rest}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** A running time in whole seconds, as a live counter shows it: `42s`, `3m 05s`, `1h 02m`. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  if (total < 60) return `${total}s`;
  const m = Math.floor(total / 60);
  if (m < 60) return `${m}m ${String(total % 60).padStart(2, "0")}s`;
  return `${Math.floor(m / 60)}h ${String(m % 60).padStart(2, "0")}m`;
}

export function formatCost(usd: number | null | undefined, estimated = false): string {
  // Nothing worth showing below a hundredth of a cent (e.g. an interrupted turn).
  if (usd == null || usd < 0.00005) return "";
  const amount = usd < 0.01 ? `$${usd.toFixed(4)}` : `$${usd.toFixed(2)}`;
  return estimated ? `≈${amount} estimated` : amount;
}

export function formatTokens(tokens: number | null | undefined): string {
  if (tokens == null) return "";
  if (tokens >= 999_500) return `${(tokens / 1_000_000).toFixed(tokens >= 99_950_000 ? 0 : 1)}M tokens`;
  return tokens >= 1000 ? `${(tokens / 1000).toFixed(tokens >= 10000 ? 0 : 1)}k tokens` : `${tokens} tokens`;
}

export function relativeTime(iso: string | null | undefined, now = Date.now()): string {
  if (!iso) return "";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 60) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.round(h / 24);
  return `${d}d ago`;
}

export function clockTime(iso: string): string {
  const t = new Date(iso);
  if (Number.isNaN(t.getTime())) return "";
  return t.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

/** Repos of a workspace's own worktrees (not the ones agents created). */
/** The name a workspace is shown with: its title, else its folder name. */
export function workspaceName(workspace: Pick<WorkspaceInfo, "title" | "slug">): string {
  return workspace.title || workspace.slug;
}

export function workspaceRepos(workspace: WorkspaceInfo): string[] {
  return workspace.worktrees
    .filter((w) => !w.removed && w.origin === "workspace")
    .map((w) => w.repo);
}

/** Repos of a workspace including archived worktrees, for archived workspaces. */
export function workspaceRepoNames(workspace: WorkspaceInfo): string[] {
  const names = workspace.worktrees.filter((w) => w.origin === "workspace").map((w) => w.repo);
  return [...new Set(names)];
}

export function linearUrl(identifier: string, url: string | null | undefined): string {
  return url ?? `https://linear.app/issue/${identifier}`;
}
