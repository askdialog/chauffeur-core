import type { WorkspaceInfo } from "@chauffeur/protocol";
import { Cog, Play, Square, TerminalSquare } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { processLines, type ProcessView } from "@/lib/workspaceLive";
import { client, toast } from "@/state/app";
import { useWorkspaceSelector, type WorkspaceSession } from "@/state/session";
import { Badge, Button, cx, EmptyState, Spinner } from "./ui";

export function ProcessesPanel({ workspace, session }: { workspace: WorkspaceInfo; session: WorkspaceSession }) {
  const liveProcesses = useWorkspaceSelector(session, (s) => s.processes);
  const processOrder = useWorkspaceSelector(session, (s) => s.processOrder);
  const worktreesChanged = useWorkspaceSelector(session, (s) => s.worktreesChanged);
  const [scripts, setScripts] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [starting, setStarting] = useState<string | null>(null);
  const ready = workspace.state === "ready";

  const refresh = useCallback(async () => {
    try {
      const result = await client.call("process.list", { workspace: workspace.id });
      session.mergeProcesses(result.processes);
      setScripts(result.scripts);
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
      setScripts([]);
    }
  }, [workspace.id, session]);

  useEffect(() => {
    void refresh();
  }, [refresh, worktreesChanged]);

  const processes = processOrder.map((id) => liveProcesses[id]).filter(Boolean).reverse();
  const current = (selected && liveProcesses[selected]) || processes.find((p) => p.running) || processes[0] || null;

  const run = async (script: string) => {
    const [repo, ...rest] = script.split(":");
    const name = rest.join(":") || undefined;
    setStarting(script);
    try {
      const info = await client.call("process.run", { workspace: workspace.id, repo, name });
      session.mergeProcesses([info]);
      setSelected(info.id);
    } catch (err) {
      toast(`Could not start ${script}: ${errorMessage(err)}`, "error");
    } finally {
      setStarting(null);
    }
  };

  const retrySetup = async (process: ProcessView) => {
    if (!process.repo) return;
    setStarting(process.id);
    try {
      const info = await client.call("process.setup", { workspace: workspace.id, process: process.id });
      session.mergeProcesses([info]);
      setSelected(info.id);
    } catch (error) {
      toast(`Could not restart ${process.name}: ${errorMessage(error)}`, "error");
    } finally { setStarting(null); }
  };

  const stop = async (process: ProcessView) => {
    try {
      const info = await client.call("process.stop", { process: process.id });
      session.mergeProcesses([info]);
    } catch (err) {
      toast(`Could not stop ${process.name}: ${errorMessage(err)}`, "error");
    }
  };

  const runningScript = (script: string) =>
    processes.find((p) => p.running && p.kind === "run" && p.name === script);

  return (
    <div className="flex h-full min-h-0">
      <aside className="flex w-[300px] shrink-0 flex-col border-r border-line">
        <div className="border-b border-line p-3">
          <div className="mb-2 text-[11px] font-medium uppercase tracking-wide text-faint">Run scripts</div>
          {scripts == null ? (
            <Spinner />
          ) : scripts.length === 0 ? (
            <div className="text-[12px] text-faint">
              {error ?? "No run script: declare them in each repo's .chauffeur/settings.toml ([scripts.run])."}
            </div>
          ) : (
            <div className="space-y-1" data-testid="run-scripts">
              {scripts.map((script) => {
                const running = runningScript(script);
                return (
                  <div key={script} className="flex items-center gap-2">
                    <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-muted" title={script}>
                      {script}
                    </span>
                    {running ? (
                      <Button size="sm" variant="danger" onClick={() => void stop(running)}>
                        <Square size={9} fill="currentColor" /> Stop
                      </Button>
                    ) : (
                      <Button size="sm" onClick={() => void run(script)} busy={starting === script} disabled={!ready}>
                        <Play size={10} fill="currentColor" /> Run
                      </Button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto py-1">
          <div className="px-3 pb-1 pt-2 text-[11px] font-medium uppercase tracking-wide text-faint">Processes</div>
          {processes.length === 0 && <div className="px-3 text-[12px] text-faint">Nothing has run yet.</div>}
          {processes.map((process) => (
            <button
              key={process.id}
              onClick={() => setSelected(process.id)}
              className={cx(
                "flex w-full items-center gap-2 px-3 py-1.5 text-left",
                current?.id === process.id ? "bg-hover" : "hover:bg-hover/50",
              )}
              data-process={process.name}
            >
              {process.kind === "run" ? (
                <TerminalSquare size={12} className="shrink-0 text-faint" />
              ) : (
                <Cog size={12} className="shrink-0 text-faint" />
              )}
              <span className="min-w-0 flex-1 truncate text-[12.5px] text-fg">{process.name}</span>
              <ProcessBadge process={process} />
            </button>
          ))}
        </div>
      </aside>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        {current ? (
          <ProcessOutput process={current} onStop={() => void stop(current)} onRetry={current.kind === "setup" && current.repo && !current.running ? () => void retrySetup(current) : undefined} retrying={starting === current.id} />
        ) : (
          <EmptyState icon={<TerminalSquare size={22} />} title="No output">
            Setup, run and archive scripts of this workspace show their output here.
          </EmptyState>
        )}
      </div>
    </div>
  );
}

function ProcessBadge({ process }: { process: ProcessView }) {
  if (process.running)
    return (
      <Badge tone="working">
        <span className="h-1.5 w-1.5 animate-pulse-dot rounded-full bg-working" /> running
      </Badge>
    );
  if (process.exitCode === 0) return <Badge tone="success">exit 0</Badge>;
  if (process.exitCode == null) return <Badge>stopped</Badge>;
  return <Badge tone="danger">exit {process.exitCode}</Badge>;
}

function ProcessOutput({ process, onStop, onRetry, retrying }: { process: ProcessView; onStop: () => void; onRetry?: () => void; retrying: boolean }) {
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const output = process.output;
  const lines = useMemo(() => processLines(output), [output]);
  useEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [lines]);
  return (
    <>
      <div className="flex h-9 shrink-0 items-center gap-2 border-b border-line px-3">
        <span className="text-[12.5px] font-medium text-fg">{process.name}</span>
        <span className="truncate font-mono text-[11.5px] text-faint" title={process.command}>
          {process.command}
        </span>
        <span className="ml-auto" />
        <ProcessBadge process={process} />
        {onRetry && <Button size="sm" onClick={onRetry} busy={retrying}>Retry setup</Button>}
        {process.running && (
          <Button size="sm" variant="danger" onClick={onStop}>
            <Square size={9} fill="currentColor" /> Stop
          </Button>
        )}
      </div>
      <div
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
        className="min-h-0 flex-1 overflow-auto bg-[#0c0c0e] px-3 py-2 font-mono text-[11.5px] leading-[17px] selectable"
        data-testid="process-output"
      >
        {lines.length === 0 ? (
          <span className="text-faint">{process.running ? "Waiting for output…" : "No output."}</span>
        ) : (
          lines.map((line, index) => (
            <div key={index} className={cx("whitespace-pre-wrap break-all", line.stream === "stderr" ? "text-[#f5a3a3]" : "text-[#d4d4d8]")}>
              {line.line || " "}
            </div>
          ))
        )}
      </div>
    </>
  );
}
