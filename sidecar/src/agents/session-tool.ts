// An exploration's way to a declared login (docs/adr/session-ensure-tool-in-explorations.md): its
// agent has no `chauffeur` CLI and its sandbox keeps the daemon's socket out, so the sidecar serves
// `session_ensure`, which does what `chauffeur session ensure <name>` does for a workspace chat:
// answers at once while the session holds, else asks the user to log in, in this chat, and waits.

import type { JsonValue, LoginSessionInfo } from "../protocol.ts";
import { errorMessage } from "../log.ts";
import { CHAUFFEUR_TOOLS, type ToolText } from "./repo-tools.ts";

export const SESSION_ENSURE = "session_ensure";
export const SESSION_ENSURE_TOOL = `mcp__${CHAUFFEUR_TOOLS}__${SESSION_ENSURE}`;

export const SESSION_ENSURE_DESCRIPTION =
  "Make sure a login this exploration's work needs holds (an AWS SSO session, an internal CLI's), as the workspace's " +
  "instructions name it for `chauffeur session ensure <name>`, which this exploration cannot run: use this tool instead. " +
  "It returns at once while the session holds; otherwise it asks the user to log in, in this chat, and returns once " +
  "they did, or fails when they declined or did not answer within 15 minutes. Never log in yourself.";

export const SESSION_ENSURE_NAME = "The session's name, as the repo's .chauffeur/settings.toml declares it (e.g. aws-sso).";

export const SESSION_ENSURE_SCHEMA: JsonValue = {
  type: "object",
  properties: { name: { type: "string", description: SESSION_ENSURE_NAME } },
  required: ["name"],
  additionalProperties: false,
};

/** What the tool needs from the daemon, scoped to the chat. */
export interface SessionToolHost {
  sessionEnsure(name: string): Promise<LoginSessionInfo>;
}

export async function sessionEnsure(host: SessionToolHost, input: unknown, options: { signal?: AbortSignal } = {}): Promise<ToolText> {
  const fields = (typeof input === "object" && input !== null ? input : {}) as Record<string, unknown>;
  const name = typeof fields.name === "string" ? fields.name.trim() : "";
  if (!name) return { text: "Name the session in `name`, as the workspace's instructions do (e.g. aws-sso).", error: true };
  // An interrupt stops the wait here; the request stays in the chat for the user.
  const given = new Promise<ToolText>((resolve) => {
    if (options.signal?.aborted) resolve(givenUp(name));
    options.signal?.addEventListener("abort", () => resolve(givenUp(name)), { once: true });
  });
  const ensured = host.sessionEnsure(name).then(
    (session): ToolText => ({
      text: `${session.label} (${session.name}) holds${session.expires_at ? ` until ${session.expires_at}` : ""}: go on.`,
      error: false,
    }),
    (error): ToolText => ({ text: `Chauffeur could not ensure the session ${name}: ${errorMessage(error)}`, error: true }),
  );
  return Promise.race([ensured, given]);
}

function givenUp(name: string): ToolText {
  return { text: `The call was given up before the session ${name} held.`, error: true };
}
