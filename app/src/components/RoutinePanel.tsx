// The Routine view of a routine's workspace (docs/adr/routines.md): its state and schedule, Run
// now, Activate or Pause, its file (edited here or by its refining chat, each save a version), its
// runs and its versions. The daemon owns the schedule; this view only asks.

import type { RoutineHistory, RoutineInfo, RoutineRun, WorkspaceInfo } from "@chauffeur/protocol";
import { Pause, Play, Power, RotateCcw, Sparkles } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import { scheduleText, stateText, whenText } from "@/lib/routines";
import { client, reopenChat, selectChat, setTab, toast, upsertRoutine, useApp } from "@/state/app";
import { Badge, Button, cx, Spinner } from "./ui";

const STATUS: Record<string, string> = {
  starting: "starting",
  idle: "done",
  working: "working",
  needs_input: "waits for you",
  error: "failed",
  closed: "closed",
};

export function RoutinePanel({ workspace, routine }: { workspace: WorkspaceInfo; routine: RoutineInfo }) {
  const [history, setHistory] = useState<RoutineHistory | null>(null);
  const [draft, setDraft] = useState(routine.file);
  // The file the draft started from: a newer version replaces an unedited draft only.
  const [base, setBase] = useState(routine.file);
  const [busy, setBusy] = useState<"run" | "state" | "save" | null>(null);
  // A run's chat changing status (it ends, it asks) shows in its row.
  const runStatuses = useApp((s) =>
    Object.values(s.chats)
      .filter((chat) => chat.workspace_id === workspace.id)
      .map((chat) => `${chat.id}:${chat.status}:${chat.closed}`)
      .join(","),
  );

  const refresh = useCallback(async () => {
    try {
      setHistory(await client.call("routine.history", { routine: routine.id }));
    } catch (error) {
      toast(`Could not read the routine's runs: ${errorMessage(error)}`, "error");
    }
  }, [routine.id]);
  useEffect(() => {
    void refresh();
  }, [refresh, routine.version, routine.last_run?.chat_id, runStatuses]);
  const edited = draft !== base;
  const newer = routine.file !== base;
  const takeCurrent = () => {
    setBase(routine.file);
    setDraft(routine.file);
  };
  // A new version (from the refining chat) replaces the file shown, unless it is being edited.
  useEffect(() => {
    if (!newer || edited) return;
    setBase(routine.file);
    setDraft(routine.file);
  }, [newer, edited, routine.file]);

  const act = async (what: "run" | "state" | "save", call: () => Promise<void>, failed: string) => {
    setBusy(what);
    try {
      await call();
    } catch (error) {
      toast(`${failed}: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(null);
    }
  };
  const runNow = () =>
    act(
      "run",
      async () => {
        const run = await client.call("routine.run", { routine: routine.id });
        selectChat(workspace.id, run.chat_id);
        setTab(workspace.id, "chat");
      },
      `Could not run ${routine.spec.name}`,
    );
  const setActive = (active: boolean) =>
    act(
      "state",
      async () => upsertRoutine(await client.call("routine.set_active", { routine: routine.id, active })),
      `Could not ${active ? "activate" : "pause"} ${routine.spec.name}`,
    );
  const save = () =>
    act(
      "save",
      async () => {
        const saved = await client.call("routine.edit", { routine: routine.id, file: draft });
        upsertRoutine(saved);
        setBase(saved.file);
      },
      "Could not save the routine's file",
    );
  const openRun = (run: RoutineRun) => {
    if (run.status === null) return;
    if (run.closed) void reopenChat(run.chat_id);
    else selectChat(workspace.id, run.chat_id);
    setTab(workspace.id, "chat");
  };
  const openRefine = () => {
    if (!routine.refine_chat) return;
    const chat = useApp.getState().chats[routine.refine_chat];
    if (chat?.closed) void reopenChat(chat.id);
    else selectChat(workspace.id, routine.refine_chat);
    setTab(workspace.id, "chat");
  };

  const now = new Date();
  const active = routine.state === "active";
  const ready = workspace.state === "ready";
  return (
    <div className="h-full overflow-y-auto" data-testid="routine-panel">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 px-6 py-5">
        <header className="flex flex-wrap items-start gap-3">
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2">
              <h2 className="truncate text-[15px] font-semibold text-fg">{routine.spec.name}</h2>
              <Badge tone={active ? "success" : routine.state === "paused" ? "attention" : "neutral"}>
                {stateText(routine, now)}
              </Badge>
            </div>
            <p className="mt-1 text-[12px] text-muted">
              {scheduleText(routine.spec.schedule)}
              {active && routine.next_run_at && ` · next run ${whenText(routine.next_run_at, now)}`}
              {!active && " · runs only when you run it"} · {routine.spec.agent}
              {routine.spec.model ? ` ${routine.spec.model}` : ""} · {routine.spec.permission_mode.replace("_", " ")}
            </p>
            {routine.error && <p className="mt-1 text-[12px] text-danger">The last scheduled run could not start: {routine.error}</p>}
          </div>
          <div className="flex shrink-0 items-center gap-1.5">
            {routine.refine_chat && (
              <Button variant="ghost" size="sm" onClick={openRefine} title="The chat whose agent edits this routine's file">
                <Sparkles size={12} /> Refine
              </Button>
            )}
            <Button size="sm" busy={busy === "run"} disabled={!ready || busy != null} onClick={() => void runNow()}>
              <Play size={12} /> Run now
            </Button>
            {active ? (
              <Button size="sm" busy={busy === "state"} disabled={busy != null} onClick={() => void setActive(false)}>
                <Pause size={12} /> Pause
              </Button>
            ) : (
              <Button
                variant="primary"
                size="sm"
                busy={busy === "state"}
                disabled={busy != null}
                onClick={() => void setActive(true)}
                title="Run it on its schedule; a run missed while the Mac sleeps is caught up once"
              >
                <Power size={12} /> Activate
              </Button>
            )}
          </div>
        </header>

        <section className="flex flex-col gap-2">
          <div className="flex items-center gap-2">
            <h3 className="text-[12px] font-semibold uppercase tracking-wide text-faint">File · version {routine.version}</h3>
            <div className="ml-auto flex items-center gap-1.5">
              {edited && (
                <Button variant="ghost" size="sm" onClick={takeCurrent}>
                  Discard
                </Button>
              )}
              <Button size="sm" variant={edited ? "primary" : "secondary"} disabled={!edited || busy != null} busy={busy === "save"} onClick={() => void save()}>
                Save as version {routine.version + 1}
              </Button>
            </div>
          </div>
          {edited && newer && (
            <p className="flex items-center gap-2 text-[12px] text-attention">
              Version {routine.version} was saved while you were editing; saving yours makes a version after it.
              <Button variant="ghost" size="xs" onClick={takeCurrent}>
                Load version {routine.version}
              </Button>
            </p>
          )}
          <textarea
            aria-label="The routine's file"
            spellCheck={false}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            className="min-h-[260px] w-full resize-y rounded-md border border-line bg-raised p-3 font-mono text-[12px] leading-relaxed text-fg outline-none focus:border-line-strong"
          />
        </section>

        <section className="flex flex-col gap-1">
          <h3 className="text-[12px] font-semibold uppercase tracking-wide text-faint">Runs</h3>
          {history == null ? (
            <Spinner />
          ) : history.runs.length === 0 ? (
            <p className="text-[12px] text-muted">No run yet: run it by hand, then tell its refining chat what was wrong.</p>
          ) : (
            history.runs.map((run) => (
              <button
                key={run.chat_id}
                type="button"
                disabled={run.status === null}
                onClick={() => openRun(run)}
                className="flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12.5px] hover:bg-hover disabled:opacity-60"
              >
                <span className="w-32 shrink-0 text-fg">{whenText(run.started_at, now)}</span>
                <span className="w-24 shrink-0 text-muted">{run.trigger === "manual" ? "by hand" : "on schedule"}</span>
                <span className="w-10 shrink-0 text-faint">v{run.version}</span>
                <span
                  className={cx(
                    "shrink-0",
                    run.status === "needs_input" ? "text-attention" : run.status === "error" ? "text-danger" : "text-muted",
                  )}
                >
                  {run.status === null ? "deleted" : STATUS[run.status]}
                </span>
                {run.last_text && <span className="min-w-0 flex-1 truncate text-faint">{run.last_text}</span>}
              </button>
            ))
          )}
        </section>

        <section className="flex flex-col gap-1">
          <h3 className="text-[12px] font-semibold uppercase tracking-wide text-faint">Versions</h3>
          {history?.versions.map((version) => (
            <div key={version.version} className="flex items-center gap-2 px-2 py-1 text-[12.5px]">
              <span className="w-10 shrink-0 text-fg">v{version.version}</span>
              <span className="flex-1 text-muted">{whenText(version.created_at, now)}</span>
              {version.version !== routine.version && (
                <Button variant="ghost" size="xs" onClick={() => setDraft(version.file)} title="Put this version in the editor; save to make it current">
                  <RotateCcw size={10} /> Load
                </Button>
              )}
            </div>
          ))}
        </section>
      </div>
    </div>
  );
}
