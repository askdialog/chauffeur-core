// The agent's tasks running now, above the composer: background commands, subagents, workflows.
// Each shows how long it has run and its last step, and can be stopped (`chat.stop_task`). A
// subagent's own tasks sit under it, indented, with a guide line.

import { ChevronDown, ChevronRight, Square, Terminal, Users, Workflow } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { type TaskView, taskStep } from "@/lib/chat";
import { formatElapsed } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import { foldedSummary, MAX_TASK_INDENT, nestTasks, taskLines, tasksHeader } from "@/lib/tasks";
import { client, toast } from "@/state/app";
import { Button, cx } from "../ui";

export function TasksPanel({
  chatId,
  tasks,
  readOnly,
  onOpen,
}: {
  chatId: string;
  tasks: TaskView[];
  readOnly: boolean;
  /** Opens a subagent's conversation (Claude chats). */
  onOpen?: (task: TaskView) => void;
}) {
  const [open, setOpen] = useState(true);
  const [stopping, setStopping] = useState<ReadonlySet<string>>(() => new Set());
  const now = useNow(tasks.length > 0);
  const trees = useMemo(() => nestTasks(tasks), [tasks]);
  if (tasks.length === 0) return null;
  const header = tasksHeader(trees);
  const parents = new Set(tasks.flatMap((task) => (task.parentTaskId ? [task.parentTaskId] : [])));

  const stop = async (taskId: string) => {
    setStopping((current) => new Set(current).add(taskId));
    try {
      await client.call("chat.stop_task", { chat: chatId, task_id: taskId });
    } catch (error) {
      toast(`Could not stop the task: ${errorMessage(error)}`, "error");
      setStopping((current) => {
        const next = new Set(current);
        next.delete(taskId);
        return next;
      });
    }
  };

  return (
    <div className="mx-auto w-full max-w-[860px] px-5 pb-1.5" data-testid="tasks">
      <div className="rounded-lg border border-line bg-raised">
        <button
          type="button"
          onClick={() => setOpen(!open)}
          aria-expanded={open}
          className={cx("flex h-7 w-full items-center gap-1.5 px-2.5 text-left text-[11.5px] text-faint hover:text-muted", open && "border-b border-line")}
        >
          {open ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          <span className="h-1.5 w-1.5 rounded-full bg-working" />
          <span className="text-muted" title={header.title ?? undefined}>
            {header.label}
          </span>
          {!open && <span className="min-w-0 flex-1 truncate">· {foldedSummary(trees)}</span>}
        </button>
        {open && (
          <div className="max-h-44 overflow-y-auto py-0.5">
            {taskLines(trees).map(({ task, depth }) => (
              <TaskRow
                key={task.taskId}
                task={task}
                depth={depth}
                hasOwnTasks={parents.has(task.taskId)}
                now={now}
                stopping={stopping.has(task.taskId)}
                onStop={readOnly ? null : () => void stop(task.taskId)}
                onOpen={onOpen && task.kind === "subagent" ? () => onOpen(task) : null}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** Left padding of a row, and how far each level of nesting moves it right. */
const ROW_PAD = 10;
const INDENT = 18;

function TaskRow({
  task,
  depth,
  hasOwnTasks,
  now,
  stopping,
  onStop,
  onOpen,
}: {
  task: TaskView;
  /** How deep it sits under the subagents that started it (0 at the top level). */
  depth: number;
  /** Tasks it started run under it. */
  hasOwnTasks: boolean;
  now: number;
  stopping: boolean;
  onStop: (() => void) | null;
  /** A subagent's line opens its conversation. */
  onOpen: (() => void) | null;
}) {
  const started = Date.parse(task.startedAt);
  const step = taskStep(task);
  const Icon = task.kind === "command" ? Terminal : task.kind === "subagent" ? Users : Workflow;
  const what = task.kind === "command" ? "Command" : task.kind === "subagent" ? "Subagent" : "Task";
  const Body = onOpen ? "button" : "div";
  const levels = Math.min(depth, MAX_TASK_INDENT);
  return (
    <div
      className="group relative flex min-h-8 items-center gap-2 py-1 pr-2.5 text-[12.5px]"
      style={{ paddingLeft: ROW_PAD + levels * INDENT }}
      data-task={task.taskId}
      data-depth={depth}
    >
      {/* A guide line under each ancestor's icon: rows of one subtree join into a rail. */}
      {Array.from({ length: levels }, (_, level) => (
        <span key={level} aria-hidden className="absolute inset-y-0 w-px bg-line" style={{ left: ROW_PAD + level * INDENT + 6 }} />
      ))}
      <Icon size={13} className="shrink-0 text-muted" aria-label={what} />
      <Body
        className={cx("min-w-0 flex-1 text-left", onOpen && "rounded hover:bg-hover/60")}
        {...(onOpen ? { type: "button" as const, onClick: onOpen, title: "Open its conversation", "data-testid": "open-subagent" } : {})}
      >
        <div className="flex min-w-0 items-center gap-1.5">
          <span className="truncate text-fg" title={task.description}>
            {task.description}
          </span>
          {task.subagentType && <span className="shrink-0 rounded bg-hover px-1 text-[10.5px] text-muted">{task.subagentType}</span>}
          {!task.background && <span className="shrink-0 text-[10.5px] text-faint" title="The agent waits for it">foreground</span>}
        </div>
        {step && (
          <div className="truncate text-[11px] text-faint" title={step}>
            {step}
          </div>
        )}
      </Body>
      <span className="shrink-0 text-[11px] tabular-nums text-faint" title={`Started at ${new Date(started).toLocaleTimeString()}`}>
        {Number.isNaN(started) ? "" : formatElapsed(now - started)}
      </span>
      {onStop && (
        <Button size="xs" variant="ghost" busy={stopping} onClick={onStop} title={hasOwnTasks ? OWN_TASKS_ON_STOP : `Stop this ${what.toLowerCase()}`} data-testid="stop-task">
          <Square size={8} fill="currentColor" /> Stop
        </Button>
      )}
    </div>
  );
}

/**
 * Stopping a subagent with tasks of its own, as Claude Code does it (seen 2026-10-18): the
 * commands it left running stop with it, the subagents it started go on.
 */
const OWN_TASKS_ON_STOP = "Stop this subagent: the commands it left running stop with it, the subagents it started keep running";

/** The time now, every second while `live`. */
function useNow(live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [live]);
  return now;
}
