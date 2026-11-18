import type { AgentAskParams, AgentForkParams, AgentLoginResult, AgentModel, AgentRetitleParams, AgentUsageResult, NativeAccountInfo, SessionStartParams, SlashCommand } from "../../protocol.ts";
import { type Logger, silentLogger } from "../../log.ts";
import { VIBE_VERSION } from "../../version.ts";
import type { Agent, AgentSession, SessionHost } from "../agent.ts";
import { spawnAcp } from "../acp/connection.ts";
import { INSTRUCTIONS, cleanTitle, request, retitleRequest } from "../title.ts";
import { traceTo } from "../trace.ts";
import { listVibeModels, readVibeAccount, readVibeUsage, vibeAsk, vibeFork, vibeLogin, vibeLoginCancel } from "./account.ts";
import { resolveVibeExecutable } from "./executable.ts";
import { VibeSession, type VibeSessionDeps } from "./session.ts";

/** The model that names work, when the account offers it (else Vibe's default). */
const QUICK_MODEL = "mistral-medium-latest";

/** Mistral Vibe through `vibe-acp` (docs/adr/mistral-vibe-third-agent.md). */
export class VibeAgent implements Agent {
  readonly kind = "vibe" as const;
  readonly versions = { "vibe-acp": VIBE_VERSION };
  private readonly deps: VibeSessionDeps;

  constructor(
    deps: Partial<VibeSessionDeps> = {},
    private readonly log: Logger = silentLogger,
  ) {
    const traceFile = process.env.CHAUFFEUR_SIDECAR_TRACE;
    this.deps = {
      spawn: spawnAcp,
      resolveExecutable: (env) => resolveVibeExecutable(env),
      baseEnv: process.env,
      ...(traceFile ? { trace: traceTo(traceFile) } : {}),
      ...deps,
    };
  }

  createSession(params: SessionStartParams, host: SessionHost): AgentSession {
    return new VibeSession(params, host, this.deps);
  }

  listModels(): Promise<AgentModel[]> {
    return listVibeModels(this.deps, this.log);
  }

  /** Vibe's commands run inside its own turns; Chauffeur offers none of them as `/name`. */
  async listCommands(): Promise<SlashCommand[]> {
    return [];
  }

  async title(text: string): Promise<string> {
    const title = cleanTitle(await vibeAsk(this.deps, this.log, `${INSTRUCTIONS} Never use tools or read files.\n\n${request(text)}`, { model: QUICK_MODEL, effort: "low", timeoutMs: 30_000 }));
    if (!title) throw new Error("an empty title");
    return title;
  }

  async retitle(params: AgentRetitleParams): Promise<string> {
    const title = cleanTitle(
      await vibeAsk(this.deps, this.log, `${INSTRUCTIONS} Never use tools or read files.\n\n${retitleRequest(params.conversation, params.current)}`, {
        model: QUICK_MODEL,
        effort: "low",
        timeoutMs: 30_000,
      }),
    );
    const kept = title.toLowerCase() === "keep" || title.toLowerCase() === params.current?.toLowerCase();
    return params.current && kept ? params.current : title;
  }

  ask(params: AgentAskParams): Promise<string> {
    return vibeAsk(this.deps, this.log, `${params.instructions}\n\nNever use tools or read files.\n\n${params.request}`, {
      model: params.model,
      effort: params.effort,
      timeoutMs: params.timeout_ms,
    });
  }

  usage(): Promise<AgentUsageResult> {
    return readVibeUsage(this.deps, this.log);
  }

  account(): Promise<NativeAccountInfo> {
    return readVibeAccount(this.deps, this.log);
  }

  login(ended: (success: boolean, error: string | null) => void): Promise<AgentLoginResult> {
    return vibeLogin(this.deps, this.log, ended);
  }

  loginCancel(): void {
    vibeLoginCancel();
  }

  fork(params: AgentForkParams): Promise<string> {
    return vibeFork(this.deps, this.log, params);
  }
}
