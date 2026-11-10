// Each repo's own `.codex/config.toml` in a Codex chat (decided with Justin 2026-10-18).
//
// Codex reads project config from `.codex/config.toml` in every folder from the project root
// (found by `.git`) down to the cwd. A chat starts in the workspace folder, above the repos and not
// a git repository, so Codex would read none of the repos' own config (their MCP servers, their
// settings). Claude chats get every repo's `.mcp.json` merged by the daemon; for Codex the sidecar
// asks Codex itself what it would load in each repo (`config/read` with the repo's worktree as
// cwd) and hands the enabled layers to the thread as its config.
//
// Trust stays Codex's: a repo's layer is enabled only when Codex trusts the repo (its worktree or
// its main checkout in the user's `config.toml [projects]`, or a managed config), exactly as when
// Codex starts in that repo; an untrusted repo's layer is left out, and Chauffeur never marks a
// repo trusted. The layer comes from Codex already cleaned of what a project may not set
// (`model_provider`, `notify`…) and with its relative paths resolved against the repo.
//
// Two repos declaring one server: the first repo's (in the workspace's order) is kept whole, as for
// Claude's merged `.mcp.json`; for any other setting, the first repo that sets it wins. A repo's
// layer then lies over the user's `config.toml` key by key, as it does when Codex starts in it.

import { basename, join } from "node:path";
import { existsSync, realpathSync } from "node:fs";
import type { JsonValue } from "../../protocol.ts";
import type { CodexConfigLayer, CodexConfigRead } from "../mcp-env.ts";
import type { AppServer } from "./app-server.ts";

type Table = { [key: string]: JsonValue };

/**
 * Keys of a repo's layer not handed to the thread. The thread's config is a layer of session flags,
 * which Codex reads before deciding trust and project roots, and trusts as the user's own:
 * - `projects`, `project_root_markers`: a repo would decide the trust and root of other folders;
 * - `bypass_hook_trust`: an app-server override that runs untrusted hooks;
 * - `hooks`: Codex runs a hook only once the user trusted it by its source; handed over, a repo's
 *   hooks would come from the session flags and need a trust Chauffeur must not give.
 */
export const NOT_CARRIED = ["projects", "project_root_markers", "bypass_hook_trust", "hooks"] as const;

/** A repo's enabled layer, as Codex loads it in the repo. */
export interface RepoLayer {
  /** The repo's worktree. */
  dir: string;
  /** Its `.codex` folder. */
  folder: string;
  config: Table;
}

/** What Codex would load in each repo. */
export interface RepoLayers {
  enabled: RepoLayer[];
  /** A repo whose layer Codex keeps disabled (untrusted), with Codex's reason. */
  untrusted: Array<{ dir: string; reason: string }>;
  /** A repo whose config Codex could not read (invalid TOML in a trusted repo…). */
  failed: Array<{ dir: string; error: string }>;
}

/** Asks Codex, repo by repo, what it loads from the repo's own `.codex/config.toml`. */
export async function readRepoLayers(server: AppServer, dirs: string[]): Promise<RepoLayers> {
  const result: RepoLayers = { enabled: [], untrusted: [], failed: [] };
  for (const dir of dirs) {
    if (!existsSync(join(dir, ".codex", "config.toml"))) continue;
    let read: CodexConfigRead;
    try {
      read = (await server.request("config/read", { includeLayers: true, cwd: dir })) as CodexConfigRead;
    } catch (error) {
      result.failed.push({ dir, error: error instanceof Error ? error.message : String(error) });
      continue;
    }
    const folder = join(dir, ".codex");
    // The repo's own layer only: with an unusual `project_root_markers`, Codex may also list a
    // folder above the repo (the workspace's), which is not the repo's to give. Codex may name the
    // folder by its real path (`/private/var/…` for `/var/…`).
    const names = new Set([folder, realPath(folder)]);
    const layer = (read.layers ?? []).find((entry) => entry.name.type === "project" && names.has(entry.name.dotCodexFolder ?? ""));
    const own = layer?.config as JsonValue | undefined;
    if (!layer || !isTable(own)) continue;
    if (layer.disabledReason) {
      if (Object.keys(own).length > 0) result.untrusted.push({ dir, reason: layer.disabledReason });
      continue;
    }
    const config = carried(own);
    if (Object.keys(config).length > 0) result.enabled.push({ dir, folder, config });
  }
  return result;
}

/** The repos' layers merged, and what each repo gave of it. */
export interface MergedRepoConfig {
  config: Table;
  /** Per repo, its layer with only the servers it gave (the first repo's on a clash). */
  given: RepoLayer[];
  /** Servers two repos declare differently. */
  clashes: string[];
}

/** Merges the repos' layers, first repo first: its servers whole, its other settings key by key. */
export function mergeRepoLayers(layers: RepoLayer[]): MergedRepoConfig {
  const config: Table = {};
  const servers: Table = {};
  const owner = new Map<string, string>();
  const given: RepoLayer[] = [];
  const clashes: string[] = [];
  for (const layer of layers) {
    const { mcp_servers: own, ...rest } = layer.config;
    const mine: Table = {};
    if (isTable(own)) {
      for (const [name, server] of Object.entries(own)) {
        if (!Object.hasOwn(servers, name)) {
          servers[name] = server;
          owner.set(name, layer.dir);
          mine[name] = server;
        } else if (!sameJson(servers[name]!, server)) {
          clashes.push(`MCP server ${name} of ${basename(layer.dir)} clashes with ${basename(owner.get(name)!)}'s; the first one is kept`);
        }
      }
    }
    fillMissing(config, rest);
    given.push({ ...layer, config: { ...rest, ...(Object.keys(mine).length > 0 ? { mcp_servers: mine } : {}) } });
  }
  if (Object.keys(servers).length > 0) config.mcp_servers = servers;
  return { config, given, clashes };
}

/**
 * The thread's config overrides: the repos' merged config under Chauffeur's own (`own`, dotted
 * keys such as `hooks.state`). Codex applies each key as a dotted path, a top-level table replacing
 * whatever an earlier key put there, and a key of a map has no order: so a repo's table that holds
 * one of Chauffeur's keys is handed key by key beneath it, and Chauffeur's own value wins.
 */
export function threadOverrides(repos: Table, own: Record<string, JsonValue>): Record<string, JsonValue> {
  const out: Record<string, JsonValue> = { ...own };
  const ownKeys = Object.keys(own);
  const visit = (prefix: string, table: Table) => {
    for (const [key, value] of Object.entries(table)) {
      const path = prefix ? `${prefix}.${key}` : key;
      if (Object.hasOwn(own, path)) continue;
      const below = ownKeys.some((ownKey) => ownKey.startsWith(`${path}.`));
      if (!below) out[path] = value;
      // A key with a dot would be split by Codex: such a table under one of Chauffeur's keys is dropped.
      else if (isTable(value) && !key.includes(".")) visit(path, value);
    }
  };
  visit("", repos);
  return out;
}

/**
 * Codex's config as `config/read` gives it for the workspace folder, with the repos' layers the
 * thread was given (which `config/read` does not see: they are the thread's overrides), so that a
 * server's declaration names the repo's file. The repos' layers rank above the workspace's and the
 * user's, below the managed ones, as the thread's session flags do.
 */
export function withRepoLayers(read: CodexConfigRead, given: RepoLayer[]): CodexConfigRead {
  if (given.length === 0) return read;
  const layers = [...(read.layers ?? [])];
  const above = new Set(["legacyManagedConfigTomlFromMdm", "legacyManagedConfigTomlFromFile", "mdm", "sessionFlags"]);
  let at = layers.findIndex((layer) => !above.has(layer.name.type));
  if (at < 0) at = layers.length;
  const repoLayers: CodexConfigLayer[] = given.map((layer) => ({ name: { type: "project", dotCodexFolder: layer.folder }, config: layer.config, disabledReason: null }));
  layers.splice(at, 0, ...repoLayers);
  const config: Table = isTable(read.config as JsonValue) ? structuredClone(read.config as Table) : {};
  // Lowest first: the first repo's settings end on top.
  for (const layer of [...given].reverse()) overlay(config, layer.config);
  return { ...read, config, layers };
}

function realPath(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return path;
  }
}

function carried(config: Table): Table {
  const out: Table = { ...config };
  for (const key of NOT_CARRIED) delete out[key];
  return out;
}

/** Copies into `target` what it does not set yet, table by table. */
function fillMissing(target: Table, source: Table): void {
  for (const [key, value] of Object.entries(source)) {
    const existing = target[key];
    if (existing === undefined) target[key] = structuredClone(value);
    else if (isTable(existing) && isTable(value)) fillMissing(existing, value);
  }
}

/** Codex's merge of a layer over a lower one: tables key by key, anything else replaced. */
function overlay(target: Table, source: Table): void {
  for (const [key, value] of Object.entries(source)) {
    const existing = target[key];
    if (isTable(existing) && isTable(value)) overlay(existing, value);
    else target[key] = structuredClone(value);
  }
}

function isTable(value: JsonValue | undefined): value is Table {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function sameJson(a: JsonValue, b: JsonValue): boolean {
  return JSON.stringify(sorted(a)) === JSON.stringify(sorted(b));
}

function sorted(value: JsonValue): JsonValue {
  if (Array.isArray(value)) return value.map(sorted);
  if (isTable(value)) return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sorted(value[key]!)]));
  return value;
}
