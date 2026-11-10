// The agent's `delegate` tool (story 49), the same for every agent: a delegated chat gets a task in
// the chat's workspace, and the call returns its answer once its turn is over. The daemon picks its
// agent (the caller's own, the other for a review) and its model (from the tier). The delegated
// chat is a chat of the workspace that the user sees as a tab, where its questions are answered;
// given its id again (`chat`), it gets more work with its context. The agent's interrupt
// interrupts it; should the agent's turn end otherwise before it answered (Codex may end its turn
// while its tool calls run), it works on and the daemon sends its answer to the parent as a new
// message. Claude gets the tool from the in-process MCP server of the repo tools,
// Codex as a dynamic tool; both show as `mcp__chauffeur__delegate`. A delegated chat has no such
// tool: one level only.

import type { AgentKind, DelegateTier, JsonValue, ToolDelegateParams, ToolDelegateResult, ToolDelegateStarted } from "../protocol.ts";
import { errorMessage } from "../log.ts";
import { CHAUFFEUR_TOOLS, type ToolText } from "./repo-tools.ts";

export const DELEGATE = "delegate";
/** The tool's name as the chat shows it, whatever the agent. */
export const DELEGATE_TOOL = `mcp__${CHAUFFEUR_TOOLS}__${DELEGATE}`;

export const DELEGATE_AGENTS = ["codex", "claude"] as const satisfies readonly AgentKind[];
export const DELEGATE_TIERS = ["fast", "default", "strongest"] as const satisfies readonly DelegateTier[];

export const DELEGATE_DESCRIPTION =
  "Have another chat do a task in this Chauffeur workspace, in the same folders, in a tab the user sees (they may " +
  "answer its questions or talk to it there); the call waits until it has finished and returns its last message. " +
  "Delegate only what needs such a chat: a review or a second opinion (`review: true`), or a long or parallel piece " +
  "of work the user may want to follow; for a quick search or a small sub-task, use your own subagents when you have " +
  "them. It runs on your own agent, except a review: that goes to the other agent (Codex from Claude, Claude from " +
  "Codex), whose model reads the work with other blind spots. If that agent is signed out or out of its plan's " +
  "limits, Chauffeur gives the task to the other one and says so. Pick the model with `tier` (`fast` for quick, " +
  "simple tasks; `default`; `strongest` for hard work) and, when the task needs it, `effort`. Name `agent` or " +
  "`model` only when the user asked for one. The delegated chat knows nothing of this conversation: say in " +
  "`prompt` everything it needs (the goal, the files, what to report back). Pass `chat`, the id a previous call " +
  "returned, to give the same delegated chat more work with its context (the other fields but `prompt` are then " +
  "ignored). Until it answered, leave the files it changes alone. Should your turn end before it answered, it works " +
  "on and its answer comes to you as a new message.";

export const DELEGATE_INPUT = {
  prompt: "The task, self-contained: the delegated chat sees nothing else of this conversation.",
  review: "A review or a second opinion: it goes to the other agent than yours.",
  tier:
    "How strong a model the task needs: fast (quick, simple), default (your own model on your agent, the user's " +
    "default on the other), strongest (hard work). Default: default.",
  effort: "The model's reasoning effort, when the task needs another than its default: low, medium, high, xhigh, max (Codex also ultra).",
  agent: "codex or claude, only when the user asked for that agent. Default: yours, or the other one for a review.",
  model: "A model id, only when the user asked for that model by name; it replaces `tier`.",
  chat: "The id of a delegated chat a previous call returned, to give it more work.",
} as const;

/** The tool's input as a JSON schema (Codex's dynamic tools). */
export const DELEGATE_SCHEMA: JsonValue = {
  type: "object",
  properties: {
    prompt: { type: "string", description: DELEGATE_INPUT.prompt },
    review: { type: "boolean", description: DELEGATE_INPUT.review },
    tier: { type: "string", enum: [...DELEGATE_TIERS], description: DELEGATE_INPUT.tier },
    effort: { type: "string", description: DELEGATE_INPUT.effort },
    agent: { type: "string", enum: [...DELEGATE_AGENTS], description: DELEGATE_INPUT.agent },
    model: { type: "string", description: DELEGATE_INPUT.model },
    chat: { type: "string", description: DELEGATE_INPUT.chat },
  },
  required: ["prompt"],
  additionalProperties: false,
};

/** What the tool needs from the daemon, scoped to the chat. */
export interface DelegateHost {
  delegate(params: Omit<ToolDelegateParams, "chat_id">): Promise<ToolDelegateStarted>;
  delegateWait(child: string): Promise<ToolDelegateResult>;
  delegateStop(child: string): Promise<void>;
  delegateDetach(child: string): Promise<void>;
}

export interface DelegateOptions {
  /** The agent gave up on the call (an interrupt): the delegated chat is interrupted too. */
  signal?: AbortSignal;
  /**
   * The agent's turn ended without waiting for the call, not by an interrupt: the delegated chat
   * works on, and its answer comes to the agent as a new message.
   */
  detach?: AbortSignal;
  /** The agent's id for the call, kept on a new delegated chat for the parent's chat to link to it. */
  toolUseId?: string;
}

export async function delegate(host: DelegateHost, input: unknown, options: DelegateOptions = {}): Promise<ToolText> {
  const fields = (typeof input === "object" && input !== null ? input : {}) as Record<string, unknown>;
  const text = (key: string) => (typeof fields[key] === "string" ? fields[key].trim() : "");
  const agent = text("agent").toLowerCase();
  const tier = text("tier").toLowerCase();
  const prompt = text("prompt");
  const chat = text("chat") || null;
  if (!prompt) return { text: "Say the task in `prompt`.", error: true };
  if (!chat && agent && !isAgent(agent)) return { text: "`agent` is codex or claude; leave it out for yours.", error: true };
  if (!chat && tier && !isTier(tier)) return { text: "`tier` is fast, default or strongest.", error: true };
  if (options.signal?.aborted) return { text: "The call was withdrawn: no delegated chat was started.", error: true };
  let started: ToolDelegateStarted;
  try {
    started = await host.delegate({
      prompt,
      ...(chat
        ? { chat }
        : {
            ...(isAgent(agent) ? { agent } : {}),
            ...(fields.review === true ? { review: true } : {}),
            ...(isTier(tier) ? { tier } : {}),
            ...(text("model") ? { model: text("model") } : {}),
            ...(text("effort") ? { effort: text("effort").toLowerCase() } : {}),
          }),
      ...(options.toolUseId ? { tool_use_id: options.toolUseId } : {}),
    });
  } catch (error) {
    return { text: `Chauffeur could not give the task to a delegated chat: ${errorMessage(error)}`, error: true };
  }
  const stop = () => {
    host.delegateStop(started.chat).catch(() => {});
    return { text: `The call was withdrawn: the delegated chat ${started.chat} was interrupted before it answered.`, error: true };
  };
  const detach = async (): Promise<ToolText> => {
    try {
      await host.delegateDetach(started.chat);
    } catch (error) {
      return { text: `Chauffeur lost track of the delegated chat ${started.chat}: ${errorMessage(error)}`, error: true };
    }
    return {
      text: `The delegated chat ${started.chat} works on: its answer will come to you as a new message.`,
      error: false,
    };
  };
  /** What Chauffeur chose otherwise than asked comes first in what the agent reads. */
  const withNotice = (said: ToolText): ToolText => (started.notice ? { ...said, text: `${started.notice}\n\n${said.text}` } : said);
  if (options.signal?.aborted) return stop();
  if (options.detach?.aborted) return withNotice(await detach());
  const listeners: [AbortSignal | undefined, () => void][] = [];
  const ended = (signal: AbortSignal | undefined, value: "aborted" | "detached") =>
    new Promise<typeof value>((resolve) => {
      const listener = () => resolve(value);
      listeners.push([signal, listener]);
      signal?.addEventListener("abort", listener, { once: true });
    });
  let result: ToolDelegateResult | "aborted" | "detached";
  try {
    result = await Promise.race([host.delegateWait(started.chat), ended(options.signal, "aborted"), ended(options.detach, "detached")]);
  } catch (error) {
    return { text: `Chauffeur lost track of the delegated chat ${started.chat}: ${errorMessage(error)}`, error: true };
  } finally {
    for (const [signal, listener] of listeners) signal?.removeEventListener("abort", listener);
  }
  if (result === "aborted") return stop();
  if (result === "detached") return withNotice(await detach());
  return withNotice(answer(result));
}

function isAgent(value: string): value is AgentKind {
  return (DELEGATE_AGENTS as readonly string[]).includes(value);
}

function isTier(value: string): value is DelegateTier {
  return (DELEGATE_TIERS as readonly string[]).includes(value);
}

/** The delegated chat's answer as the agent reads it: who answered, how its turn ended, its message. */
export function answer(result: ToolDelegateResult): ToolText {
  const name = result.agent === "codex" ? "Codex" : "Claude";
  const who = `${name} (delegated chat ${result.chat}${result.title ? `, "${result.title}"` : ""})`;
  const more = `Give it more work with delegate and chat "${result.chat}".`;
  const message = result.text?.trim() ? `\n\n${result.text.trim()}` : "";
  switch (result.outcome) {
    case "success":
      return { text: `${who} answered:${message || "\n\n(no message)"}\n\n${more}`, error: false };
    case "interrupted":
      return {
        text: `${who} was interrupted before it finished${message ? `; its last message:${message}` : "."}\n\n${more}`,
        error: true,
      };
    case "error":
      return {
        text: `${who} stopped on an error${result.error ? `: ${result.error}` : ""}${message ? `\n\nIts last message:${message}` : ""}\n\n${more}`,
        error: true,
      };
    default:
      return { text: `${who} could not work on the task: ${result.error ?? "its agent did not start"}.`, error: true };
  }
}
