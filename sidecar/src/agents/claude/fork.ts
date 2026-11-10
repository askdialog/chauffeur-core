// A chat forked into a new tab: Claude Code's session copied into a new one (`forkSession`), with
// fresh message ids, in the same project folder, so that resuming the new id carries the
// conversation on without touching the original's. A fork from a turn copies it up to the turn's
// last message.

import { forkSession } from "@anthropic-ai/claude-agent-sdk";
import { errorMessage } from "../../log.ts";
import { ErrorCode, type AgentForkParams } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";

/**
 * Copies session `sessionId` of folder `dir` into a new one, up to message `upTo` (a transcript
 * uuid, inclusive) when given; resolves with its id.
 */
export type SessionForker = (sessionId: string, dir: string, upTo?: string) => Promise<string>;

export const sdkFork: SessionForker = async (sessionId, dir, upTo) =>
  (await forkSession(sessionId, { dir, ...(upTo ? { upToMessageId: upTo } : {}) })).sessionId;

/** Session ids are UUIDs, and become file names. */
const SESSION_ID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export async function claudeFork(fork: SessionForker, params: AgentForkParams): Promise<string> {
  if (!SESSION_ID.test(params.session_id)) throw new RpcError(ErrorCode.BAD_REQUEST, `not a Claude Code session id: ${params.session_id}`);
  if (params.up_to !== undefined && !SESSION_ID.test(params.up_to)) {
    throw new RpcError(ErrorCode.BAD_REQUEST, `not a Claude Code message id: ${params.up_to}`);
  }
  try {
    return await fork(params.session_id, params.cwd, params.up_to);
  } catch (error) {
    throw new RpcError(ErrorCode.UNAVAILABLE, `Claude Code could not fork its session: ${errorMessage(error)}`);
  }
}
