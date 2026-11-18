// Which `vibe-acp` runs. As for Codex, the user's own: CHAUFFEUR_VIBE_PATH, else `vibe-acp` on the
// agent's PATH (`uv tool install mistral-vibe` puts it in ~/.local/bin; Mistral also publishes a
// standalone `vibe-acp` with each GitHub release). A session logs a warning when the version
// `initialize` reports differs from VIBE_VERSION, the one Chauffeur was tried with.

import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { ErrorCode } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";

export const VIBE_INSTALL_HINT = "install it with `uv tool install mistral-vibe`, or set CHAUFFEUR_VIBE_PATH";

export interface VibeLookup {
  which: (command: string, options: { PATH: string }) => string | null;
  exists: (path: string) => boolean;
  home: string;
}

const runtime: VibeLookup = { which: (command, options) => Bun.which(command, options), exists: existsSync, home: homedir() };

/** Resolves the vibe-acp binary from the agent's environment. Throws an RpcError when missing. */
export function resolveVibeExecutable(env: Record<string, string | undefined>, lookup: VibeLookup = runtime): string {
  const override = env.CHAUFFEUR_VIBE_PATH;
  if (override) {
    if (!lookup.exists(override)) {
      throw new RpcError(ErrorCode.UNAVAILABLE, `CHAUFFEUR_VIBE_PATH points to a missing file: ${override}`);
    }
    return override;
  }
  const found = lookup.which("vibe-acp", { PATH: env.PATH ?? "" });
  if (found) return found;
  // uv's tool folder, often missing from a GUI app's PATH.
  const uvTool = join(lookup.home, ".local", "bin", "vibe-acp");
  if (lookup.exists(uvTool)) return uvTool;
  throw new RpcError(ErrorCode.UNAVAILABLE, `Mistral Vibe is not installed: no \`vibe-acp\` on PATH (${VIBE_INSTALL_HINT})`);
}
