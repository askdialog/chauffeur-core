// Chauffeur's chat settings → Codex app-server parameters.

import { join } from "node:path";
import type { Attachment, JsonValue, PermissionMode } from "../../protocol.ts";
import { attachmentNote, isImage } from "../attachments.ts";
import { DESIGN_BROWSER, designBrowserCommand } from "../design-browser.ts";
import type { ApprovalsReviewer, AskForApproval, SandboxMode, SandboxPolicy, UserInput } from "./protocol.ts";

/**
 * Permission modes. Codex has no per-tool callback: an approval policy decides when to ask, a
 * sandbox bounds what runs without asking, and a reviewer (the user, or Codex's auto review)
 * answers what asks.
 *
 * - auto: Codex's closest match to Claude Code's auto mode, auto review: the workspace-write
 *   sandbox of `default`, and what needs to leave it (a commit in a worktree, whose gitdir is
 *   outside the sandbox; network) goes to a reviewer agent that approves or denies it by risk
 *   instead of to the user. Its denials show as notices. Where an admin's requirements forbid
 *   auto review, Codex falls back to asking the user. The agent must ask to leave the sandbox
 *   for a commit to work (SANDBOX_NOTE tells it so).
 * - bypass_permissions: never ask, no sandbox. Codex's workspace-write sandbox cannot commit in a
 *   worktree (the gitdir is outside its writable roots) without an approval, so full access is
 *   what never asks.
 * - default: ask when a command needs to leave the sandbox (`on-request`); the sandbox lets
 *   commands and edits write inside the workspace folder (and /tmp), not elsewhere, and has no
 *   network. Commits in the repos need an approval, since their gitdir is outside the workspace.
 * - accept_edits: the same as default. Codex already applies edits inside the workspace without
 *   asking under `on-request`; no Codex mode asks for commands but not for edits.
 * - plan: a read-only sandbox (`on-request`, so writing needs an approval), and each message
 *   carries a note asking for a plan rather than changes. Codex's own plan mode is experimental.
 *
 * An exploration (read-only) runs in the read-only sandbox and never asks to leave it: a command
 * that needs more simply fails. Its instructions already say to explain, so no plan note.
 *
 * A design chat (also one inside an exploration: the design wins) writes its design folder and
 * nothing else, whatever mode the client asks for (see `designSandboxPolicy`): it never asks to
 * leave its sandbox, so a patch or a command that writes elsewhere fails.
 *
 * A chat whose agent is given the `chauffeur` CLI reaches the daemon's socket from every sandbox
 * (see `daemonSocket`); an exploration does not.
 */
export interface ModeSettings {
  approvalPolicy: AskForApproval;
  sandbox: SandboxMode;
  approvalsReviewer: ApprovalsReviewer;
}

export const PERMISSION_MODES: Record<PermissionMode, ModeSettings> = {
  auto: { approvalPolicy: "on-request", sandbox: "workspace-write", approvalsReviewer: "auto_review" },
  bypass_permissions: { approvalPolicy: "never", sandbox: "danger-full-access", approvalsReviewer: "user" },
  default: { approvalPolicy: "on-request", sandbox: "workspace-write", approvalsReviewer: "user" },
  accept_edits: { approvalPolicy: "on-request", sandbox: "workspace-write", approvalsReviewer: "user" },
  plan: { approvalPolicy: "on-request", sandbox: "read-only", approvalsReviewer: "user" },
};

const READ_ONLY: ModeSettings = { approvalPolicy: "never", sandbox: "read-only", approvalsReviewer: "user" };

const DESIGN: ModeSettings = { approvalPolicy: "never", sandbox: "workspace-write", approvalsReviewer: "user" };

/**
 * Added to the developer instructions of a thread that may write: a small model that hits the
 * sandbox on `git commit` tends to give up rather than ask to leave it.
 */
export const SANDBOX_NOTE =
  "When a command fails because the sandbox blocks it (for example `git commit` in a worktree, whose git directory is outside the sandbox), run it again with escalated permissions instead of giving up. Do the same for a `chauffeur` command that says a sandbox blocked its connection to the Chauffeur daemon.";

/** The approval policy and sandbox of a session, an exploration's or a design chat's whatever its mode. */
export function modeSettings(mode: PermissionMode, readOnly: boolean, design = false): ModeSettings {
  return design ? DESIGN : readOnly ? READ_ONLY : PERMISSION_MODES[mode];
}

export const PLAN_NOTE =
  "[Chauffeur plan mode: investigate and propose a plan. Do not modify files or run commands that change anything until the user approves the plan.]";

/** The sandbox policy of a mode, for `turn/start` overrides (thread start takes the mode name). */
export function sandboxPolicy(mode: PermissionMode, writableRoots: string[], readOnly = false): SandboxPolicy {
  switch (modeSettings(mode, readOnly).sandbox) {
    case "danger-full-access":
      return { type: "dangerFullAccess" };
    case "read-only":
      return { type: "readOnly", networkAccess: false };
    case "workspace-write":
      return { type: "workspaceWrite", writableRoots, networkAccess: false, excludeTmpdirEnvVar: false, excludeSlashTmp: false };
  }
}

/**
 * A design chat's sandbox: the design folder as its only writable root, without the /tmp and
 * $TMPDIR that workspace-write adds by default (a workspace under either would be writable whole),
 * and no network. Codex checks a patch's paths against these roots and runs it inside the sandbox,
 * which also stops a write through a symlink or a hard link out of the folder.
 */
export function designSandboxPolicy(designDir: string): SandboxPolicy {
  return { type: "workspaceWrite", writableRoots: [designDir], networkAccess: false, excludeTmpdirEnvVar: true, excludeSlashTmp: true };
}

/**
 * A design chat's thread config: the same sandbox for the thread as for its turns, over what the
 * user's or a repo's config would add (writable roots, the network), and its own browser. Its MCP
 * tools are approved by config: a design chat never asks, and `never` refuses any MCP tool that
 * would need an approval.
 */
export function designThreadConfig(designDir: string): Record<string, JsonValue> {
  return {
    "sandbox_workspace_write.writable_roots": [],
    "sandbox_workspace_write.network_access": false,
    "sandbox_workspace_write.exclude_slash_tmp": true,
    "sandbox_workspace_write.exclude_tmpdir_env_var": true,
    [`mcp_servers.${DESIGN_BROWSER}`]: { ...designBrowserCommand(designDir), default_tools_approval_mode: "approve" },
  };
}

/** A user message and its images, with the plan note in plan mode (not in an exploration). */
export function userInputs(text: string, attachments: Attachment[], mode: PermissionMode, cwd: string | null = null, readOnly = false): UserInput[] {
  const body = mode === "plan" && !readOnly ? `${PLAN_NOTE}\n\n${text}` : text;
  // The text names the attachments' files, even when the user wrote nothing; only images go as images.
  const inputs: UserInput[] = [{ type: "text", text: `${body}${attachmentNote(attachments, cwd)}`.trimStart(), text_elements: [] }];
  for (const attachment of attachments) if (isImage(attachment)) inputs.push({ type: "localImage", path: attachment.path });
  return inputs;
}

/**
 * The daemon's Unix socket, which the `chauffeur` CLI connects to, for a chat whose agent is given
 * the CLI: the daemon puts `CHAUFFEUR_HOME` in its environment (`env`, the session's own, not the
 * sidecar's). An exploration's agent gets neither (and is read-only besides): no socket.
 *
 * macOS only: Codex's Linux sandbox (seccomp) has no allowlist of sockets, only all of them or
 * none; there `chauffeur` says that the sandbox blocked it, and the agent asks to leave it.
 */
export function daemonSocket(
  env: Record<string, string | undefined> | undefined,
  readOnly: boolean,
  platform: NodeJS.Platform = process.platform,
): string | undefined {
  const home = env?.CHAUFFEUR_HOME;
  return home && !readOnly && platform === "darwin" ? join(home, "chauffeurd.sock") : undefined;
}

/**
 * Codex's sandbox has no network, Unix sockets included, so `chauffeur` fails in it (EPERM). Codex
 * 0.162 allows a Unix socket only through its network proxy (`features.network_proxy`, which takes
 * an allowlist of sockets), and starts that proxy only for a thread that begins with network in its
 * sandbox. So the thread begins in the workspace-write sandbox with network on and the proxy
 * allowing the daemon's socket and no domain: its commands reach that socket, any other traffic is
 * refused (the proxy answers 403, nothing else leaves the sandbox), and the first turn sets the
 * mode's own sandbox (no network, or read-only, or none), keeping the proxy and its socket for the
 * thread's life. A command run outside the sandbox (escalated, or in bypass mode) is not proxied.
 */
export function daemonSocketConfig(socket: string): Record<string, JsonValue> {
  return {
    "features.network_proxy": { enabled: true, unix_sockets: { [socket]: "allow" } },
    "sandbox_workspace_write.network_access": true,
  };
}

/**
 * Whether a config (Codex's, or the repos' given to the thread) already gives the workspace-write
 * sandbox the network, which reaches the daemon's socket too: the proxy would then close what the
 * user opened, so the thread is left as it was.
 */
export function sandboxNetworkOpen(config: unknown): boolean {
  if (!config || typeof config !== "object") return false;
  const sandbox = (config as Record<string, unknown>).sandbox_workspace_write;
  return !!sandbox && typeof sandbox === "object" && (sandbox as Record<string, unknown>).network_access === true;
}

/** Config overrides of a thread: extra writable roots, the trust of Chauffeur's own hook, the daemon's socket. */
export function threadConfig(
  additionalDirectories: string[],
  hookTrust: Record<string, JsonValue> | undefined,
  readOnly = false,
  socket?: string,
): Record<string, JsonValue> {
  const config: Record<string, JsonValue> = {};
  if (additionalDirectories.length > 0 && !readOnly) config["sandbox_workspace_write.writable_roots"] = additionalDirectories;
  if (hookTrust) config["hooks.state"] = hookTrust;
  if (socket) Object.assign(config, daemonSocketConfig(socket));
  return config;
}

/**
 * `codex app-server` arguments: Chauffeur's PreToolUse hook, as a session flag (config override),
 * which the sidecar then trusts per thread (see CodexSession).
 */
export function appServerArgs(hookCommand: string | undefined): string[] {
  if (!hookCommand) return [];
  const hook = `{type="command", command=${JSON.stringify(hookCommand)}}`;
  return ["-c", `hooks.PreToolUse=[{matcher="Bash", hooks=[${hook}]}]`];
}

/** A chat's model that Ollama serves (story 52) is `ollama/<name>`. */
const LOCAL_MODEL_PREFIX = "ollama/";
/** The context the daemon gives Ollama's models (its `OLLAMA_CONTEXT_LENGTH`). */
export const LOCAL_CONTEXT_WINDOW = 65_536;

/** The name Ollama knows a local model by; none for a hosted model. */
export function localModelName(model: string | null | undefined): string | undefined {
  return model?.startsWith(LOCAL_MODEL_PREFIX) ? model.slice(LOCAL_MODEL_PREFIX.length) : undefined;
}

/**
 * A thread's model: a local one by its name on Codex's built-in Ollama provider, which a thread
 * takes only when it starts or resumes; a hosted one as is.
 */
export function threadModel(model: string | null | undefined): { model?: string; modelProvider?: string } {
  const local = localModelName(model);
  if (local) return { model: local, modelProvider: "ollama" };
  return model ? { model } : {};
}

/** Notifications the sidecar never uses: the app-server skips them. */
export const OPTED_OUT_NOTIFICATIONS = [
  "account/updated",
  "account/rateLimits/updated",
  "remoteControl/status/changed",
  "thread/status/changed",
  "turn/diff/updated",
  "hook/started",
  "item/fileChange/outputDelta",
  "fs/changed",
];
