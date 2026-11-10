// Owns the SDK's native child. Iterator completion or an abort request is not process exit.
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import type { SpawnOptions } from "@anthropic-ai/claude-agent-sdk";
import { ErrorCode } from "../../protocol.ts";
import { RpcError } from "../../rpc/peer.ts";
import { settlesWithin } from "../../util/settles-within.ts";

export class ClaudeProcess {
  private child: ChildProcessWithoutNullStreams | undefined;
  private exited: Promise<void> | undefined;
  private gone = false;
  private sealed = false;

  constructor(private readonly stderr: (text: string) => void) {}

  get pid(): number | undefined { return this.child?.pid; }

  readonly spawn = (options: SpawnOptions): ChildProcessWithoutNullStreams => {
    if (this.sealed || this.child) throw new RpcError(ErrorCode.CONFLICT, "Claude launch is closed; reconcile the session before starting again");
    const child = spawn(options.command, options.args, {
      cwd: options.cwd, env: options.env, signal: options.signal, stdio: ["pipe", "pipe", "pipe"], windowsHide: true,
    });
    this.child = child;
    this.exited = new Promise((resolve) => {
      child.once("exit", () => { this.gone = true; resolve(); });
      child.on("error", () => {
        // A spawn failure with no PID created no native process. AbortError with a PID
        // is merely a signal request: the exit event still has to arrive.
        if (child.pid === undefined) { this.gone = true; resolve(); }
      });
    });
    child.stderr.on("data", (data: Buffer) => this.stderr(data.toString()));
    return child;
  };

  /** Seals late SDK launches and confirms root exit. Detached descendants remain unproven. */
  async stop(graceMs: number): Promise<void> {
    this.sealed = true;
    const child = this.child;
    if (!child || !this.exited) return;
    if (await settlesWithin(this.exited, graceMs)) return;
    if (!this.gone) child.kill("SIGKILL");
    if (!(await settlesWithin(this.exited, 2_000))) {
      throw new RpcError(ErrorCode.UNAVAILABLE, "Claude process exit is unconfirmed; keep the workspace fenced and retry stopping the session");
    }
  }
}
