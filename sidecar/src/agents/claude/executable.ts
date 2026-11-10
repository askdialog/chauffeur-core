// Where the Claude Code CLI binary comes from.
//
// The SDK finds its native CLI with require.resolve on its platform package
// (@anthropic-ai/claude-agent-sdk-<os>-<arch>/claude). That works under `bun run`, but not inside a
// `bun build --compile` binary, whose modules live in the $bunfs virtual filesystem. So
// scripts/build.ts embeds the platform binary as an asset, and at runtime we extract it to a real
// path (content-addressed under the per-user Claude temp dir, reused across runs) with the SDK's
// own `extractFromBunfs` helper.
//
// Order: CHAUFFEUR_CLAUDE_PATH, then the embedded binary, then the SDK's own resolution.

import { existsSync } from "node:fs";
import { extractFromBunfs } from "@anthropic-ai/claude-agent-sdk/extract";
import { ErrorCode } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";

/** File name of the Claude Code binary embedded by scripts/build.ts. */
export const EMBEDDED_CLAUDE = "claude";

export interface ExecutableEnvironment {
  env: Record<string, string | undefined>;
  standalone: boolean;
  /** Names of the files embedded in the standalone binary. */
  embeddedNames: () => string[];
  exists: (path: string) => boolean;
  extract: (bunfsPath: string) => string;
}

const runtime: ExecutableEnvironment = {
  env: process.env,
  standalone: Bun.isStandaloneExecutable,
  embeddedNames: () => Bun.embeddedFiles.map((file) => (file as Blob & { name?: string }).name ?? ""),
  exists: existsSync,
  extract: extractFromBunfs,
};

let extracted: string | undefined;

/**
 * The path to pass as `pathToClaudeCodeExecutable`, or undefined to let the SDK resolve its own.
 * Throws an RpcError when no usable binary exists.
 */
export function resolveClaudeExecutable(environment: ExecutableEnvironment = runtime): string | undefined {
  const override = environment.env.CHAUFFEUR_CLAUDE_PATH;
  if (override) {
    if (!environment.exists(override)) {
      throw new RpcError(ErrorCode.UNAVAILABLE, `CHAUFFEUR_CLAUDE_PATH points to a missing file: ${override}`);
    }
    return override;
  }
  if (!environment.standalone) return undefined;
  if (extracted && environment.exists(extracted)) return extracted;
  const name = environment.embeddedNames().find((file) => file === EMBEDDED_CLAUDE || file.endsWith(`/${EMBEDDED_CLAUDE}`));
  if (!name) {
    throw new RpcError(
      ErrorCode.UNAVAILABLE,
      "this sidecar binary embeds no Claude Code executable: build it with `bun run build` or set CHAUFFEUR_CLAUDE_PATH",
    );
  }
  const path = environment.extract(`/$bunfs/root/${name}`);
  if (path.includes("$bunfs")) {
    throw new RpcError(ErrorCode.UNAVAILABLE, "could not extract the embedded Claude Code executable (see stderr)");
  }
  extracted = path;
  return path;
}

/** Forgets the extracted path (tests). */
export function resetExtractedExecutable(): void {
  extracted = undefined;
}
