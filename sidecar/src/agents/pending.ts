// Permission requests, MCP confirmations and questions waiting for `session.respond`, whatever the agent: each one
// is announced by an event, settled by the user's answer (or cancelled), and closed by a
// `request_resolved` event.

import { randomUUID } from "node:crypto";
import { ErrorCode, type ChatEvent, type RequestResponse } from "../protocol.ts";
import { RpcError } from "../rpc/peer.ts";

export type Outcome = RequestResponse | { type: "cancelled" };
export type RequestKind = "permission" | "question" | "elicitation";

/** The tool call a permission request asks for, or the server an MCP confirmation comes from. */
export interface Subject {
  toolName: string;
  input: Record<string, unknown>;
  /**
   * What Always allow on this request would allow (Codex: its session approval's keys, or the
   * confirmation's server and message, as rules).
   */
  rules?: string[];
}

/** What a request's `request_resolved` event says beyond its outcome. */
export type Resolution = (outcome: Outcome) => { rules?: string[] };

interface Pending {
  kind: RequestKind;
  settle: (outcome: Outcome) => void;
  /** Whether the request outlives the agent's turn (a background subagent's), asked when it ends. */
  outlivesTurn?: () => boolean;
  subject?: Subject;
  resolution?: Resolution;
  validate?: (response: RequestResponse) => void;
}

export interface AskOptions {
  /** The agent gave up on the request: it is cancelled. */
  signal?: AbortSignal;
  /** Asked when the turn is interrupted: whether the request goes on regardless (see `cancelTurn`). */
  outlivesTurn?: () => boolean;
  /** The tool call a permission request (or the confirmation) is for, for `allowCovered`. */
  subject?: Subject;
  resolution?: Resolution;
  validate?: (response: RequestResponse) => void;
}

export class PendingRequests {
  private readonly pending = new Map<string, Pending>();

  constructor(
    private readonly emit: (event: ChatEvent) => void,
    private readonly newId: () => string = randomUUID,
  ) {}

  get size(): number {
    return this.pending.size;
  }
  /** A request is waiting for the user. */
  waiting(): boolean {
    return this.pending.size > 0;
  }


  /** Emits the event built for a fresh request id and resolves with the answer. */
  async ask(kind: RequestKind, event: (requestId: string) => ChatEvent, options: AskOptions = {}): Promise<Outcome> {
    const { signal, outlivesTurn, subject, resolution, validate } = options;
    const requestId = this.newId();
    const outcome = new Promise<Outcome>((resolve) => {
      this.pending.set(requestId, { kind, settle: resolve, outlivesTurn, subject, resolution, validate });
    });
    this.emit(event(requestId));
    const onAbort = () => this.cancel(requestId);
    if (signal?.aborted) onAbort();
    else signal?.addEventListener("abort", onAbort, { once: true });
    try {
      return await outcome;
    } finally {
      signal?.removeEventListener("abort", onAbort);
    }
  }

  /** Answers a pending request. Throws NOT_FOUND for an unknown (or already answered) id. */
  respond(requestId: string, response: RequestResponse): void {
    const pending = this.pending.get(requestId);
    if (!pending) throw new RpcError(ErrorCode.NOT_FOUND, `no pending request ${requestId}`);
    if ((response.type === "elicitation") !== (pending.kind === "elicitation")) {
      throw new RpcError(ErrorCode.BAD_REQUEST, `request ${requestId} needs its own response type`);
    }
    pending.validate?.(response);
    if (response.type === "answer" && pending.kind !== "question") {
      throw new RpcError(ErrorCode.BAD_REQUEST, `request ${requestId} is a permission request: answer allow or deny`);
    }
    this.settle(requestId, response);
  }

  cancel(requestId: string): void {
    this.settle(requestId, { type: "cancelled" });
  }

  /** Cancels every pending request (stop, the agent process gone, a Codex interrupt). */
  cancelAll(): void {
    for (const requestId of [...this.pending.keys()]) this.cancel(requestId);
  }

  /**
   * Cancels the requests an interrupt ends: those of the turn, not those that outlive it (a
   * background subagent's, which an interrupt spares and which stay answerable).
   */
  cancelTurn(): void {
    for (const [requestId, pending] of [...this.pending]) {
      if (!pending.outlivesTurn?.()) this.cancel(requestId);
    }
  }

  /**
   * Allows the open permission requests and MCP confirmations that a rule the user just allowed
   * always covers, as Claude Code's terminal does with the calls still waiting: `covering` names
   * the rules, or none. `always`: each is answered as allowed always (Codex, whose session approval
   * is per request).
   */
  allowCovered(covering: (subject: Subject) => string[] | undefined, always = false): void {
    for (const [requestId, pending] of [...this.pending]) {
      if (pending.kind === "question" || !pending.subject) continue;
      const rules = covering(pending.subject);
      if (rules === undefined || rules.length === 0) continue;
      // A confirmation gets a rule only when it has nothing to fill in.
      const outcome: Outcome =
        pending.kind === "elicitation" ? { type: "elicitation", action: "accept", content: {}, always } : { type: "allow", always };
      this.settle(requestId, outcome, { rules });
    }
  }

  private settle(requestId: string, outcome: Outcome, extra?: { rules?: string[] }): void {
    const pending = this.pending.get(requestId);
    if (!pending) return;
    this.pending.delete(requestId);
    const { rules } = extra ?? pending.resolution?.(outcome) ?? {};
    this.emit({
      type: "request_resolved",
      request_id: requestId,
      outcome: outcomeName(outcome),
      ...(rules && rules.length > 0 ? { rules } : {}),
    });
    pending.settle(outcome);
  }
}

/** The `request_resolved.outcome` of an answer. */
export function outcomeName(outcome: Outcome): "allowed" | "denied" | "answered" | "cancelled" {
  switch (outcome.type) {
    case "allow":
      return "allowed";
    case "deny":
      return "denied";
    case "answer":
      return "answered";
    case "elicitation":
      switch (outcome.action) {
        case "accept": return "allowed";
        case "decline": return "denied";
        case "cancel": return "cancelled";
      }
    case "cancelled":
      return "cancelled";
  }
}
