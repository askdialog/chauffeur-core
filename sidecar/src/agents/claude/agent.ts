import { query } from "@anthropic-ai/claude-agent-sdk";
import { readNativeAccount } from "./account.ts";
import type {
  AgentAskParams,
  AgentBriefParams,
  AgentForkParams,
  AgentLoginResult,
  AgentModel,
  AgentRetitleParams,
  AgentSubagentMessagesParams,
  AgentSubagentMessagesResult,
  AgentUsageResult,
  SessionStartParams,
  SlashCommand,
} from "../../protocol.ts";
import { CLAUDE_AGENT_SDK_VERSION } from "../../version.ts";
import type { Agent, AgentSession, SessionHost } from "../agent.ts";
import { traceTo } from "../trace.ts";
import { resolveClaudeExecutable } from "./executable.ts";
import { listClaudeCommands } from "./commands.ts";
import { listClaudeModels } from "./models.ts";
import { ClaudeSession, type ClaudeSessionDeps } from "./session.ts";
import { claudeSubagentMessages, sdkTranscripts, type TranscriptReader } from "./subagents.ts";
import { claudeAsk } from "./ask.ts";
import { claudeBrief } from "./brief.ts";
import { claudeFork, sdkFork, type SessionForker } from "./fork.ts";
import { claudeRetitle, claudeTitle } from "./title.ts";
import { claudeLogin, claudeLoginCancel, claudeLoginCode } from "./login.ts";
import { claudeUsage } from "./usage.ts";

/** Claude Code through the official Claude Agent SDK. */
export class ClaudeAgent implements Agent {
  readonly kind = "claude" as const;
  readonly versions = { "claude-agent-sdk": CLAUDE_AGENT_SDK_VERSION };
  private readonly deps: ClaudeSessionDeps;
  private readonly transcripts: TranscriptReader;

  private readonly forker: SessionForker;

  constructor(deps: Partial<ClaudeSessionDeps> = {}, transcripts: TranscriptReader = sdkTranscripts, forker: SessionForker = sdkFork) {
    this.transcripts = transcripts;
    this.forker = forker;
    const traceFile = process.env.CHAUFFEUR_SIDECAR_TRACE;
    this.deps = {
      query: (args) => query(args),
      resolveExecutable: () => resolveClaudeExecutable(),
      baseEnv: process.env,
      ...(traceFile ? { trace: traceTo(traceFile) } : {}),
      ...deps,
    };
  }

  createSession(params: SessionStartParams, host: SessionHost): AgentSession {
    return new ClaudeSession(params, host, this.deps);
  }

  listModels(): Promise<AgentModel[]> {
    return listClaudeModels(this.deps);
  }

  listCommands(cwd: string, directories: string[]): Promise<SlashCommand[]> {
    return listClaudeCommands(this.deps, cwd, directories);
  }

  account() { return readNativeAccount(this.deps); }

  usage(): Promise<AgentUsageResult> {
    return claudeUsage(this.deps);
  }

  login(ended: (success: boolean, error: string | null) => void, headless: boolean): Promise<AgentLoginResult> {
    return claudeLogin(this.deps, ended, headless);
  }

  loginCode(code: string): Promise<void> {
    return claudeLoginCode(code);
  }

  loginCancel(): void {
    claudeLoginCancel();
  }

  /** A short title for the work `text` describes (see claudeTitle). */
  title(text: string): Promise<string> {
    return claudeTitle(this.deps, text);
  }

  /** A workspace's title again from its conversation (see claudeRetitle). */
  retitle(params: AgentRetitleParams): Promise<string> {
    return claudeRetitle(this.deps, params);
  }

  /** One question in one turn (see claudeAsk). */
  ask(params: AgentAskParams): Promise<string> {
    return claudeAsk(this.deps, params);
  }

  /** A design brief from a chat's conversation (see claudeBrief). */
  brief(params: AgentBriefParams): Promise<string> {
    return claudeBrief(this.deps, params);
  }

  subagentMessages(params: AgentSubagentMessagesParams): Promise<AgentSubagentMessagesResult> {
    return claudeSubagentMessages(this.transcripts, params);
  }

  /** The session copied into a new one (see claudeFork). */
  fork(params: AgentForkParams): Promise<string> {
    return claudeFork(this.forker, params);
  }
}
