// Claude Code's model catalog (the SDK's `supportedModels`) → AgentModel.

import { homedir } from "node:os";
import type { ModelInfo, Options, SDKUserMessage } from "@anthropic-ai/claude-agent-sdk";
import type { AgentModel } from "../../protocol.ts";
import { errorMessage } from "../../log.ts";
import { AsyncQueue } from "../../util/async-queue.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import type { ClaudeSessionDeps, QueryLike } from "./session.ts";

/** The row Claude Code lists for "no model chosen"; it names the model it resolves to. */
const DEFAULT_ROW = "default";
const LIST_TIMEOUT_MS = 60_000;

/**
 * Drops the "Default (recommended)" row and marks the model it resolves to as the default
 * instead, so each model appears once under its own name.
 */
export function claudeModels(rows: ModelInfo[]): AgentModel[] {
  const defaultRow = rows.find((row) => row.value === DEFAULT_ROW);
  let defaultMarked = false;
  return rows
    .filter((row) => row.value !== DEFAULT_ROW)
    .map((row) => {
      const resolved = row.resolvedModel ?? null;
      const isDefault = !defaultMarked && defaultRow?.resolvedModel !== undefined && resolved === defaultRow.resolvedModel;
      if (isDefault) defaultMarked = true;
      return {
        id: row.value,
        resolved,
        name: row.displayName,
        description: row.description,
        efforts: row.supportsEffort === false ? [] : [...(row.supportedEffortLevels ?? [])],
        default_effort: null,
        supports_fast: row.supportsFastMode === true,
        is_default: isDefault,
      };
    });
}

/**
 * Starts Claude Code without a prompt just to read its catalog, which follows the user's settings
 * (an `availableModels` narrows it) and account.
 */
export async function listClaudeModels(deps: ClaudeSessionDeps): Promise<AgentModel[]> {
  const rows = await probeClaude(deps, { cwd: homedir(), settingSources: ["user"] }, (query) => query.supportedModels(), "list its models");
  return claudeModels(rows);
}

/**
 * Starts Claude Code without a prompt to ask it something (its catalog, its commands), then
 * closes it. No message is sent, so nothing is billed.
 */
export async function probeClaude<T>(
  deps: ClaudeSessionDeps,
  options: Options,
  read: (query: QueryLike) => Promise<T>,
  what: string,
): Promise<T> {
  const input = new AsyncQueue<SDKUserMessage>();
  const abortController = new AbortController();
  const executable = deps.resolveExecutable();
  const query = deps.query({
    prompt: input,
    options: {
      env: { CLAUDE_AGENT_SDK_CLIENT_APP: `chauffeur-sidecar/${SIDECAR_VERSION}`, ...deps.baseEnv },
      abortController,
      ...(executable ? { pathToClaudeCodeExecutable: executable } : {}),
      ...options,
    },
  });
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      read(query),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error(`no answer after ${LIST_TIMEOUT_MS / 1000}s`)), LIST_TIMEOUT_MS);
      }),
    ]);
  } catch (error) {
    throw new Error(`Claude Code could not ${what}: ${errorMessage(error)}`);
  } finally {
    clearTimeout(timer);
    input.end();
    query.close();
    abortController.abort();
  }
}
