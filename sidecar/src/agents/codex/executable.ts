// Which `codex` runs. Unlike Claude's single native binary, the Codex npm package ships a 300 MB
// platform folder (binary, bundled ripgrep, helper binaries) that is not worth embedding: the
// sidecar runs CHAUFFEUR_CODEX_PATH, else `codex` on the agent's PATH. The generated protocol
// types come from CODEX_APP_SERVER_VERSION; the session logs a warning when the version differs.

import { existsSync } from "node:fs";
import { ErrorCode } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";

export interface CodexLookup {
  which: (command: string, options: { PATH: string }) => string | null;
  exists: (path: string) => boolean;
}

const runtime: CodexLookup = { which: (command, options) => Bun.which(command, options), exists: existsSync };

/** Resolves the codex binary from the agent's environment. Throws an RpcError when missing. */
export function resolveCodexExecutable(env: Record<string, string | undefined>, lookup: CodexLookup = runtime): string {
  const override = env.CHAUFFEUR_CODEX_PATH;
  if (override) {
    if (!lookup.exists(override)) {
      throw new RpcError(ErrorCode.UNAVAILABLE, `CHAUFFEUR_CODEX_PATH points to a missing file: ${override}`);
    }
    return override;
  }
  const found = lookup.which("codex", { PATH: env.PATH ?? "" });
  if (!found) {
    throw new RpcError(ErrorCode.UNAVAILABLE, "codex is not installed: no `codex` on PATH (install it, or set CHAUFFEUR_CODEX_PATH)");
  }
  return found;
}
