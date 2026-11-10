// Which environment variables a broken MCP server lacks, and where it is declared, so that the
// app guides the user (export it, reload the environment, or remove the server) rather than show
// an error (decided with Justin 2026-10-18). Variables are named, never their values.
//
// Codex names the variable in its error ("Environment variable SLACK_MCP_TOKEN for MCP server
// 'slack' is not set") only for `bearer_token_env_var`: a variable of `env_http_headers` or
// `env_vars` that is not set is dropped without a word, and the server fails on whatever it hits.
// So for Codex too the server's entry in its config (as `config/read` merges its layers: the
// user's `config.toml`, a trusted project's `.codex/config.toml`…) is checked against the
// session's environment. Claude Code never names one either: an unset `${VAR}` of
// `.mcp.json` or `~/.claude.json` stays as written and the server fails on whatever it hits
// ("'url' is not a valid URL", "Connection closed"). So for Claude the server's declaration is read
// (its status hides a stdio server's `env`) and its `${VAR}` checked against the session's
// environment.

import { readFileSync } from "node:fs";
import { join } from "node:path";

type Env = Record<string, string | undefined>;

const VARIABLE = "[A-Za-z_][A-Za-z0-9_]*";
const CODEX_MESSAGES = [
  new RegExp(`Environment variable (${VARIABLE}) for MCP server '[^']*' is (?:not set|empty)`, "g"),
  new RegExp(`env var \`?(${VARIABLE})\`? is not set`, "g"),
];
/** `${VAR}`; `${VAR:-default}` has a value whatever happens. */
const REFERENCE = new RegExp(`\\$\\{(${VARIABLE})(:-[^}]*)?\\}`, "g");

/** The variables a Codex server's error says are not set (or empty). */
export function codexMissingEnv(error: string | null | undefined): string[] {
  if (!error) return [];
  const found = new Set<string>();
  for (const pattern of CODEX_MESSAGES) {
    for (const match of error.matchAll(pattern)) found.add(match[1]!);
  }
  return [...found];
}

/** Where Codex reads a server from: its plugin, else its config's table. */
export function codexDeclaredIn(name: string, pluginId: string | null | undefined, env: Env): string {
  if (pluginId) return `the plugin ${pluginId}`;
  const home = env.CODEX_HOME ? tilde(env.CODEX_HOME, env) : "~/.codex";
  return `${home}/config.toml [mcp_servers.${tomlKey(name)}]`;
}

/** The `${VAR}` without a default that `value` (any JSON) refers to and `env` lacks. */
export function unsetReferences(value: unknown, env: Env): string[] {
  return [...new Set(references(value).filter((reference) => !reference.withDefault && !env[reference.name]).map((reference) => reference.name))];
}

/** Every variable whose `${VAR}` (with a default or not) `value` (any JSON) refers to. */
export function referencedVariables(value: unknown): string[] {
  return [...new Set(references(value).map((reference) => reference.name))];
}

function references(value: unknown): Array<{ name: string; withDefault: boolean }> {
  const found: Array<{ name: string; withDefault: boolean }> = [];
  const visit = (node: unknown) => {
    if (typeof node === "string") {
      for (const match of node.matchAll(REFERENCE)) found.push({ name: match[1]!, withDefault: match[2] !== undefined });
    } else if (Array.isArray(node)) {
      node.forEach(visit);
    } else if (node && typeof node === "object") {
      Object.values(node).forEach(visit);
    }
  };
  visit(value);
  return found;
}

/** The variables of `variables` that `env` lacks (unset or empty). */
export function unsetVariables(variables: string[], env: Env): string[] {
  return variables.filter((name) => !env[name]);
}

/** What Codex's config says of a server: only the names of its variables, and where it is. */
export interface CodexDeclaration {
  /** `bearer_token_env_var`, the variables of `env_http_headers`, `env_vars` (local ones). */
  variables: string[];
  /** The file whose table declares it: `~/.codex/config.toml [mcp_servers.slack]`. */
  where: string;
}

/** A layer of Codex's config as `config/read` lists it (highest precedence first). */
export interface CodexConfigLayer {
  name: { type: string; file?: string; dotCodexFolder?: string };
  config: unknown;
  disabledReason: string | null;
}

/** `config/read`'s answer, of which only the servers are read. */
export interface CodexConfigRead {
  config: unknown;
  layers: CodexConfigLayer[] | null;
}

/**
 * A server's entry in Codex's config, as Codex merged its layers (a trusted project's
 * `.codex/config.toml` over the user's `config.toml`, a deeper folder's over its parent's), and the
 * layer that declares it: the highest one giving its `url` or `command`, else the highest one
 * naming it. Of the config, only that entry's variable names are kept: never a value.
 */
export function codexDeclaration(name: string, read: CodexConfigRead, env: Env): CodexDeclaration | undefined {
  const entry = serverEntry(read.config, name);
  if (!entry) return undefined;
  const enabled = (read.layers ?? []).filter((layer) => !layer.disabledReason);
  const declaring =
    enabled.find((layer) => {
      const own = serverEntry(layer.config, name);
      return own !== undefined && (typeof own.url === "string" || typeof own.command === "string");
    }) ?? enabled.find((layer) => serverEntry(layer.config, name) !== undefined);
  return {
    variables: codexVariables(entry),
    where: declaring ? `${layerName(declaring, env)} [mcp_servers.${tomlKey(name)}]` : codexDeclaredIn(name, null, env),
  };
}

/** The variables a Codex server's entry takes values from, by name. */
export function codexVariables(entry: Record<string, unknown>): string[] {
  // A server started in another environment (a remote executor) reads that one's variables.
  if (typeof entry.environment_id === "string" && entry.environment_id !== "local") return [];
  const found = new Set<string>();
  const add = (value: unknown) => {
    if (typeof value === "string" && new RegExp(`^${VARIABLE}$`).test(value)) found.add(value);
  };
  add(entry.bearer_token_env_var);
  if (entry.env_http_headers && typeof entry.env_http_headers === "object") Object.values(entry.env_http_headers).forEach(add);
  if (Array.isArray(entry.env_vars)) {
    for (const variable of entry.env_vars) {
      if (typeof variable === "string") add(variable);
      else if (variable && typeof variable === "object" && (variable as { source?: unknown }).source !== "remote") add((variable as { name?: unknown }).name);
    }
  }
  return [...found];
}

function serverEntry(config: unknown, name: string): Record<string, unknown> | undefined {
  const servers = config && typeof config === "object" ? (config as Record<string, unknown>).mcp_servers : undefined;
  const entry = servers && typeof servers === "object" && Object.hasOwn(servers, name) ? (servers as Record<string, unknown>)[name] : undefined;
  return entry && typeof entry === "object" && !Array.isArray(entry) ? (entry as Record<string, unknown>) : undefined;
}

/** The file a layer comes from, for the user; a managed one (MDM, cloud) or the command line's said so. */
function layerName(layer: CodexConfigLayer, env: Env): string {
  if (layer.name.type === "project" && layer.name.dotCodexFolder) return tilde(join(layer.name.dotCodexFolder, "config.toml"), env);
  if (layer.name.file) return tilde(layer.name.file, env);
  return layer.name.type === "sessionFlags" ? "Codex's command line (-c)" : "Codex's managed configuration";
}

export interface ClaudeDeclarationContext {
  /** The session's environment, as Claude Code got it. */
  env: Env;
  /** The session's folder (the workspace's). */
  cwd: string;
  /** The workspace's repos: the `.mcp.json` the workspace's own merges. */
  directories: string[];
  /** Overridable in tests; undefined when the file is missing or unreadable. */
  readJson?: (path: string) => unknown;
}

export interface Declaration {
  /** The server as written in the file. */
  config: unknown;
  /** The file, for the user: `~/.claude.json`, `~/work/repo/.mcp.json`. */
  where: string;
}

/**
 * A Claude Code server's declaration, from the file of its scope: `~/.claude.json` (or
 * `$CLAUDE_CONFIG_DIR/.claude.json`) for user and local servers, a repo's `.mcp.json` (else the
 * workspace's) for project ones. Only that server's entry is kept from the file.
 */
export function claudeDeclaration(name: string, scope: string | undefined, context: ClaudeDeclarationContext): Declaration | undefined {
  const read = context.readJson ?? readJson;
  const servers = (file: unknown, ...path: string[]) => {
    let node: unknown = file;
    for (const key of path) node = node && typeof node === "object" ? (node as Record<string, unknown>)[key] : undefined;
    return node && typeof node === "object" && Object.hasOwn(node, name) ? (node as Record<string, unknown>)[name] : undefined;
  };
  if (scope === "user" || scope === "local") {
    const dir = context.env.CLAUDE_CONFIG_DIR ?? context.env.HOME;
    if (!dir) return undefined;
    const path = join(dir, ".claude.json");
    const file = read(path);
    const config = scope === "user" ? servers(file, "mcpServers") : servers(file, "projects", context.cwd, "mcpServers");
    if (config === undefined) return undefined;
    const where = tilde(path, context.env);
    return { config, where: scope === "user" ? where : `${where} (the servers of ${tilde(context.cwd, context.env)})` };
  }
  if (scope === "project") {
    for (const dir of [...context.directories, context.cwd]) {
      const path = join(dir, ".mcp.json");
      const config = servers(read(path), "mcpServers");
      if (config !== undefined) return { config, where: tilde(path, context.env) };
    }
  }
  return undefined;
}

function readJson(path: string): unknown {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch {
    return undefined;
  }
}

function tilde(path: string, env: Env): string {
  const home = env.HOME;
  if (home && (path === home || path.startsWith(`${home}/`))) return `~${path.slice(home.length)}`;
  return path;
}

function tomlKey(name: string): string {
  return /^[A-Za-z0-9_-]+$/.test(name) ? name : JSON.stringify(name);
}
