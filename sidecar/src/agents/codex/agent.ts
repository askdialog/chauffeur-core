import { readNativeAccount } from "./account.ts";
import type { AgentAskParams, AgentRetitleParams, AgentForkParams, AgentLoginResult, AgentModel, AgentUsageResult, SessionStartParams, SlashCommand } from "../../protocol.ts";
import { type Logger, silentLogger } from "../../log.ts";
import { CODEX_APP_SERVER_VERSION } from "../../version.ts";
import type { Agent, AgentSession, SessionHost } from "../agent.ts";
import { traceTo } from "../trace.ts";
import { spawnAppServer } from "./app-server.ts";
import { codexAsk } from "./ask.ts";
import { codexTitle, codexRetitle } from "./title.ts";
import { codexFork } from "./fork.ts";
import { resolveCodexExecutable } from "./executable.ts";
import { hookCommandLine } from "./hook.ts";
import { listCodexModels } from "./models.ts";
import { CodexSession, type CodexSessionDeps } from "./session.ts";
import { listCodexCommands } from "./skills.ts";
import { codexLogin, codexLoginCancel } from "./login.ts";
import { readCodexUsage } from "./usage.ts";

/** Codex through `codex app-server`. */
export class CodexAgent implements Agent {
  readonly kind = "codex" as const;
  readonly versions = { "codex-app-server": CODEX_APP_SERVER_VERSION };
  private readonly deps: CodexSessionDeps;

  constructor(
    deps: Partial<CodexSessionDeps> = {},
    private readonly log: Logger = silentLogger,
  ) {
    const traceFile = process.env.CHAUFFEUR_SIDECAR_TRACE;
    this.deps = {
      spawn: spawnAppServer,
      resolveExecutable: (env) => resolveCodexExecutable(env),
      baseEnv: process.env,
      hookCommand: hookCommandLine,
      ...(traceFile ? { trace: traceTo(traceFile) } : {}),
      ...deps,
    };
  }

  createSession(params: SessionStartParams, host: SessionHost): AgentSession {
    return new CodexSession(params, host, this.deps);
  }

  listModels(): Promise<AgentModel[]> {
    return listCodexModels(this.deps, this.log);
  }

  title(text: string): Promise<string> {
    return codexTitle(this.deps, this.log, text);
  }

  retitle(params: AgentRetitleParams): Promise<string> {
    return codexRetitle(this.deps, this.log, params);
  }

  ask(params: AgentAskParams): Promise<string> {
    return codexAsk(this.deps, this.log, params);
  }

  account() { return readNativeAccount(this.deps, this.log); }

  usage(): Promise<AgentUsageResult> {
    return readCodexUsage(this.deps, this.log);
  }

  login(ended: (success: boolean, error: string | null) => void, headless: boolean): Promise<AgentLoginResult> {
    return codexLogin(this.deps, this.log, ended, headless);
  }

  loginCancel(): void {
    codexLoginCancel();
  }

  /** Codex's skills: a message starting with `/name` runs one (see CodexSession). */
  listCommands(cwd: string, directories: string[]): Promise<SlashCommand[]> {
    return listCodexCommands(this.deps, this.log, cwd, directories);
  }

  /** The thread copied into a new one (see codexFork). */
  fork(params: AgentForkParams): Promise<string> {
    return codexFork(this.deps, this.log, params);
  }
}
