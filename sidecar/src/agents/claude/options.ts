// SessionStartParams → Claude Agent SDK Options.

import type {
  CanUseTool,
  EffortLevel,
  McpSdkServerConfigWithInstance,
  McpStdioServerConfig,
  Options,
  PermissionMode as SdkPermissionMode,
  Settings,
} from "@anthropic-ai/claude-agent-sdk";
import { realpathSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { ErrorCode, type PermissionMode, type SessionStartParams } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import type { Hooks } from "./hooks.ts";
import { USER_JEV_PLUGIN } from "./jev.ts";
import { DELEGATE_TOOL } from "../delegate-tool.ts";
import { DESIGN_BROWSER, designBrowserCommand } from "../design-browser.ts";
import { ADD_REPO_TOOL, CHAUFFEUR_TOOLS, LIST_REPOS_TOOL, UPDATE_PLAN_TOOL } from "../repo-tools.ts";

/**
 * Chauffeur's modes → the SDK's. `auto` is Claude Code's auto mode: a classifier runs what it
 * judges safe, denies the rest (the agent reads why), and asks through canUseTool when unsure.
 * Where auto mode is unavailable (the model, the plan, an admin or a setting turns it off) Claude
 * Code starts in `default` instead, which asks: never a mode that asks less (see ClaudeSession).
 */
export const PERMISSION_MODES: Record<PermissionMode, SdkPermissionMode> = {
  auto: "auto",
  default: "default",
  accept_edits: "acceptEdits",
  plan: "plan",
  bypass_permissions: "bypassPermissions",
};

/**
 * The tools an exploration's chats do without: notebook edits, and ExitPlanMode, which would hand
 * the session a mode that edits. Plan mode (forced by the daemon) keeps the rest read-only; Write
 * and Edit stay for the plan, and the plan guard (hooks.ts) refuses them anywhere else.
 */
export const READ_ONLY_DISALLOWED_TOOLS = ["NotebookEdit", "ExitPlanMode"];

/** An exploration without a plans folder (the daemon could not make it) writes nothing at all. */
export const READ_ONLY_NO_PLANS_DISALLOWED_TOOLS = ["Edit", "Write", "MultiEdit", "NotebookEdit", "ExitPlanMode"];

/** Where an exploration's shell commands may reach: GitHub, for `gh` reads, and nothing else. */
export const READ_ONLY_ALLOWED_DOMAINS = ["github.com", "*.github.com", "*.githubusercontent.com"];

/**
 * An exploration's Bash runs in Claude Code's OS sandbox (Seatbelt on macOS, bubblewrap on
 * Linux), the one thing that makes read-only hold whatever the command: writes stay inside the
 * workspace folder (the disposable checkouts; the clones' refs and the state folder are out of
 * reach) but out of its notes (`notesDir`, kept on archive: plans come through the Write tool and
 * the plan guard), and in the exploration's own temporary folder (`CLAUDE_CODE_TMPDIR`, from the
 * daemon); no Unix socket (so no way to the daemon), no network but GitHub, the Jev key withheld.
 * A command that fails under the sandbox is never retried outside it, and a machine without the
 * sandbox's dependencies refuses the session rather than running unsandboxed. The read-only
 * guard (hooks.ts) still refuses git and GitHub writes by name, with a reason the agent can read.
 * What a command writes is removed afterwards: the checkouts and the temporary folder at archive,
 * and at promote the user discards the checkouts' leftovers or keeps the exploration.
 */
export function readOnlySandbox(notesDir?: string): NonNullable<Options["sandbox"]> {
  return {
    enabled: true,
    failIfUnavailable: true,
    allowUnsandboxedCommands: false,
    // Off, yet in plan mode Claude Code runs a command it can sandbox without asking: accepted, as
    // what it writes is removed afterwards (decided with Justin 2026-10-18).
    autoAllowBashIfSandboxed: false,
    network: {
      allowedDomains: READ_ONLY_ALLOWED_DOMAINS,
      strictAllowlist: true,
      allowUnixSockets: [],
      allowAllUnixSockets: false,
      allowLocalBinding: false,
    },
    credentials: {
      envVars: [{ name: "TYPESAFE_API_KEY", mode: "deny" }],
    },
    ...(notesDir ? { filesystem: { denyWrite: [notesDir] } } : {}),
  };
}

/**
 * The tools a design chat does without: notebook edits, ExitPlanMode (it never plans code), and
 * EnterWorktree (a worktree is a place to change code). Write and Edit stay for its screens, and
 * the write guard (hooks.ts) refuses them anywhere but its design folder.
 */
export const DESIGN_DISALLOWED_TOOLS = ["NotebookEdit", "ExitPlanMode", "EnterWorktree"];

/** Claude's Artifact tool publishes to claude.ai: a design chat asks before each use. */
export const DESIGN_ASK_RULES = ["Artifact"];

/** Playwright MCP without a window, for the agent to check its screens (see design-browser.ts). */
export function designBrowser(designDir: string): McpStdioServerConfig {
  return { type: "stdio", ...designBrowserCommand(designDir) };
}

/**
 * Allow rules for reading a folder and everything in it, as Claude Code writes an absolute path
 * (`//abs/**`): the path as given and, when it goes through a symlink (macOS's /tmp), as resolved,
 * since Claude Code may compare either.
 */
export function notesReadRules(notesDir: string | undefined): { allow?: string[] } {
  if (!notesDir) return {};
  const paths = [notesDir];
  try {
    const real = realpathSync(notesDir);
    if (real !== notesDir) paths.push(real);
  } catch {
    // Not made yet: the path as given.
  }
  return { allow: paths.map((path) => `Read(/${path.replace(/\/+$/, "")}/**)`) };
}

/**
 * A design chat's Bash runs in the same OS sandbox as an exploration's (no Unix socket, no network
 * but GitHub, never retried outside, the Jev key withheld), with two differences. Its commands run
 * without asking: inside the sandbox they can write only the design folder, which is the session's
 * own folder (`cwd`). And the repos, which the session reads as additional directories, are denied
 * writing, so nothing a command does reaches the code.
 */
export function designSandbox(repos: string[]): NonNullable<Options["sandbox"]> {
  return {
    ...readOnlySandbox(),
    autoAllowBashIfSandboxed: true,
    filesystem: { denyWrite: [...repos] },
  };
}

/**
 * The plans folder as Claude Code's `plansDirectory`: relative to the session's folder when inside
 * it. Claude Code ignores (and falls back to ~/.claude/plans) a folder it does not find under its
 * project root, which it compares in its resolved form: an absolute path through a symlink
 * (macOS's /tmp → /private/tmp, a symlinked home) would not match, a relative one always does.
 */
export function plansDirectory(plansDir: string, cwd: string): string {
  const inside = relative(resolve(cwd), resolve(plansDir));
  return inside && inside !== ".." && !inside.startsWith(`..${sep}`) && !isAbsolute(inside) ? inside : plansDir;
}

export const EFFORT_LEVELS: readonly EffortLevel[] = ["low", "medium", "high", "xhigh", "max"];

export function effortLevel(value: string): EffortLevel {
  if (!EFFORT_LEVELS.includes(value as EffortLevel)) {
    throw new RpcError(ErrorCode.INVALID_PARAMS, `effort must be one of ${EFFORT_LEVELS.join(", ")}`);
  }
  return value as EffortLevel;
}

export interface OptionDeps {
  /** The sidecar's environment; the SDK's `env` replaces the CLI's, so it is the base. */
  baseEnv: Record<string, string | undefined>;
  /** `pathToClaudeCodeExecutable`, when the SDK cannot find its own. */
  executable: string | undefined;
  canUseTool: CanUseTool;
  hooks: Hooks;
  abortController: AbortController;
  stderr: (data: string) => void;
  /** The folder of Chauffeur's Jev plugin, written on first use. */
  jevPluginDir: () => string;
  /** Chauffeur's own tools (the repo tools, `delegate`), for every chat but a design chat's. */
  repoTools?: McpSdkServerConfigWithInstance;
}

/**
 * Chauffeur's own tools, allowed by the chat: `add_repo` asks the user itself, and a subagent
 * chat asks for what its own agent does.
 */
export const CHAUFFEUR_TOOL_RULES = [LIST_REPOS_TOOL, ADD_REPO_TOOL, DELEGATE_TOOL, UPDATE_PLAN_TOOL];

export function buildClaudeOptions(params: SessionStartParams, deps: OptionDeps): Options {
  const options: Options = {
    cwd: params.cwd,
    additionalDirectories: params.additional_directories,
    env: {
      CLAUDE_AGENT_SDK_CLIENT_APP: `chauffeur-sidecar/${SIDECAR_VERSION}`,
      ...deps.baseEnv,
      ...params.env,
    },
    settingSources: ["user", "project", "local"],
    includePartialMessages: true,
    permissionMode: PERMISSION_MODES[params.permission_mode],
    // Always allowed so that session.configure can switch a running chat to bypass permissions;
    // it only makes the mode available, the chat's own mode decides.
    allowDangerouslySkipPermissions: true,
    canUseTool: deps.canUseTool,
    hooks: deps.hooks,
    abortController: deps.abortController,
    stderr: deps.stderr,
    // Chauffeur stops tasks one by one (the tasks bar, `chauffeur stop-task`): an interrupt then
    // ends the turn only, and background subagents and workflows keep running.
    perTaskStopAffordance: true,
    // Fills subagents' task_progress `summary` (a short line every ~30 s, from a fork of the
    // subagent that reuses its prompt cache), which the tasks bar shows.
    agentProgressSummaries: true,
    // A subagent's text and thinking too, not only its tool calls: the app shows them live in the
    // subagent's view (never in the chat itself, nor in the event log).
    forwardSubagentText: true,
    // AskUserQuestion's option previews in markdown, which the app renders as it does answers
    // (never HTML from the model).
    toolConfig: { askUserQuestion: { previewFormat: "markdown" } },
    // A guess at the user's next message after each turn, offered in the empty composer. It
    // reuses the turn's prompt cache, and arrives after the turn's `result`.
    promptSuggestions: params.prompt_suggestions,
  };
  if (params.design_dir) {
    // A design chat (also one inside an exploration: the design wins) runs in auto mode, whatever
    // mode the client asks for: its guards (hooks.ts) and sandbox keep every write to its folder,
    // the classifier decides the rest. Where auto mode is unavailable the session switches to
    // acceptEdits (session.ts).
    options.permissionMode = "auto";
    options.allowDangerouslySkipPermissions = false;
    options.disallowedTools = DESIGN_DISALLOWED_TOOLS;
    options.sandbox = designSandbox(params.additional_directories);
    options.mcpServers = { [DESIGN_BROWSER]: designBrowser(params.design_dir) };
  } else if (params.read_only) {
    options.permissionMode = "plan";
    options.allowDangerouslySkipPermissions = false;
    options.disallowedTools = params.plans_dir ? READ_ONLY_DISALLOWED_TOOLS : READ_ONLY_NO_PLANS_DISALLOWED_TOOLS;
    options.sandbox = readOnlySandbox(params.notes_dir);
  }
  if (params.model) options.model = params.model;
  if (params.effort) options.effort = effortLevel(params.effort);
  // Flag-tier settings: they apply to this session only, never written to the user's settings.
  const settings: Settings = {};
  if (params.fast) settings.fastMode = true;
  // Plans in the workspace's notes (Files tab, kept on archive) rather than ~/.claude/plans; Claude
  // Code's plan mode names that folder to the model and lets it write its plan file there.
  // A design chat never plans (no ExitPlanMode, and its write guard keeps it to its design folder).
  if (params.plans_dir && !params.design_dir) settings.plansDirectory = plansDirectory(params.plans_dir, params.cwd);
  // Ask rules win over any allow rule of the user's settings: publishing a screen is approved each time.
  // A design chat reads the workspace's notes (the user's attachments, other chats' screenshots)
  // without asking, though they sit outside its working directories.
  if (params.design_dir) settings.permissions = { ask: [...DESIGN_ASK_RULES], ...notesReadRules(params.notes_dir) };
  // The repo tools (story 48), never in a design chat: allowed here so that neither a mode that asks
  // nor auto mode's classifier stands between the agent and `add_repo`'s own question.
  if (deps.repoTools && !params.design_dir) {
    options.mcpServers = { [CHAUFFEUR_TOOLS]: deps.repoTools };
    settings.permissions = { ...settings.permissions, allow: [...(settings.permissions?.allow ?? []), ...CHAUFFEUR_TOOL_RULES] };
  }
  if (params.jev) {
    options.plugins = [{ type: "local", path: deps.jevPluginDir() }];
    options.env = { ...options.env, CLAUDE_CODE_ENABLE_FUNCTION_HOOKS: "1", TYPESAFE_API_KEY: params.jev.api_key };
    // The user's own install would compact a second time, and so would Claude Code while idle.
    settings.enabledPlugins = { [USER_JEV_PLUGIN]: false };
    settings.idleCompaction = false;
  }
  if (Object.keys(settings).length > 0) options.settings = settings;
  if (params.resume_session_id) options.resume = params.resume_session_id;
  if (params.append_system_prompt) {
    options.systemPrompt = { type: "preset", preset: "claude_code", append: params.append_system_prompt };
  }
  if (deps.executable) options.pathToClaudeCodeExecutable = deps.executable;
  return options;
}
