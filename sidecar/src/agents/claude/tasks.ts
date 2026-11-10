// Claude Code's tasks (a shell command left running, a subagent, a workflow) as agent-agnostic
// task events. Shown: background tasks, and subagents even in the foreground. Hidden: the tasks
// Claude Code keeps for itself (ambient, skip_transcript), and a foreground command until it is
// moved to the background (its tool call already shows it as the agent's current step).
//
// Every task shown gets one `task_ended`: from Claude Code's notification, or, when none comes,
// from its tool result (a foreground task), the end of the turn (a foreground task cannot outlive
// it), or the end of the session (the CLI process, and its tasks with it, are gone).

import type {
  SDKMessage,
  SDKTaskNotificationMessage,
  SDKTaskProgressMessage,
  SDKTaskStartedMessage,
  SDKTaskUpdatedMessage,
} from "@anthropic-ai/claude-agent-sdk";
import type { ChatEvent, RequestAgent, TaskKind, TaskStatus } from "../../protocol.ts";

type TaskMessage = SDKTaskStartedMessage | SDKTaskProgressMessage | SDKTaskUpdatedMessage | SDKTaskNotificationMessage;

/** The SDK subtypes this class maps. */
export const TASK_SUBTYPES = new Set(["task_started", "task_progress", "task_updated", "task_notification"]);

/** Summaries are cut to this many characters. */
const SUMMARY_LIMIT = 2_000;

interface Task {
  started: Extract<ChatEvent, { type: "task_started" }>;
  /** A `task_started` was emitted for it. */
  shown: boolean;
}

export class ClaudeTasks {
  private readonly tasks = new Map<string, Task>();

  /** Whether `message` is a task message, mapped by `map`. */
  static handles(message: SDKMessage): message is TaskMessage {
    return message.type === "system" && TASK_SUBTYPES.has(message.subtype);
  }

  map(message: TaskMessage): ChatEvent[] {
    switch (message.subtype) {
      case "task_started":
        return this.started(message);
      case "task_progress": {
        const task = this.tasks.get(message.task_id);
        if (!task?.shown) return [];
        return [
          {
            type: "task_progress",
            task_id: message.task_id,
            // A subagent's progress describes its current step, not the task.
            activity: message.description && message.description !== task.started.description ? cut(message.description) : null,
            last_tool: message.last_tool_name ?? null,
            summary: message.summary ? cut(message.summary) : null,
            duration_ms: finite(message.usage?.duration_ms),
            tool_uses: finite(message.usage?.tool_uses),
          },
        ];
      }
      case "task_updated":
        return this.updated(message);
      case "task_notification":
        return this.end(message.task_id, message.status, message.summary || null);
    }
  }

  /** A tool result: a foreground task started by that call is over. */
  toolResult(toolUseId: string, isError: boolean): ChatEvent[] {
    for (const [id, task] of this.tasks) {
      if (task.started.tool_use_id === toolUseId && !task.started.background) {
        return this.end(id, isError ? "failed" : "completed", null);
      }
    }
    return [];
  }

  /** The turn ended: so did its foreground tasks. */
  turnEnded(): ChatEvent[] {
    const events: ChatEvent[] = [];
    for (const [id, task] of [...this.tasks]) {
      if (!task.started.background) events.push(...this.end(id, "completed", null));
    }
    return events;
  }

  /** The session ends: every task still running is stopped with it. */
  endAll(summary: string): ChatEvent[] {
    return [...this.tasks.keys()].flatMap((id) => this.end(id, "stopped", summary));
  }

  /** A task shown and still running: one the user can stop. */
  running(taskId: string): boolean {
    return this.tasks.get(taskId)?.shown === true;
  }

  /**
   * The subagent behind a permission request: Claude Code's agent id is its task's id. Only the
   * id when its task is not known (hidden, or not reported yet).
   */
  agent(agentId: string): RequestAgent {
    const started = this.tasks.get(agentId)?.started;
    if (!started) return { id: agentId, subagent_type: null, description: null, background: false };
    return {
      id: agentId,
      subagent_type: started.subagent_type,
      description: started.description,
      background: started.background,
    };
  }

  private started(message: SDKTaskStartedMessage): ChatEvent[] {
    if (message.ambient === true || message.skip_transcript === true) return [];
    const kind = taskKind(message.task_type);
    const started: Task["started"] = {
      type: "task_started",
      task_id: message.task_id,
      kind,
      description: message.description || message.workflow_name || message.task_type || "task",
      subagent_type: message.subagent_type ?? null,
      tool_use_id: message.tool_use_id ?? null,
      // Unset for every type but commands and subagents, which then run beside the turn.
      background: message.is_backgrounded !== false,
      parent_task_id: message.parent_task_id ?? null,
    };
    // A foreground command shows as the agent's current step already.
    const shown = !(kind === "command" && !started.background);
    this.tasks.set(message.task_id, { started, shown });
    return shown ? [started] : [];
  }

  private updated(message: SDKTaskUpdatedMessage): ChatEvent[] {
    const task = this.tasks.get(message.task_id);
    if (!task) return [];
    const patch = message.patch;
    const events: ChatEvent[] = [];
    const moved = patch.is_backgrounded === true && !task.started.background;
    if (moved || (patch.description && patch.description !== task.started.description)) {
      task.started = {
        ...task.started,
        background: task.started.background || moved,
        description: patch.description || task.started.description,
      };
      // Sent again: a client updates the task it shows.
      if (task.shown || moved) {
        task.shown = true;
        events.push(task.started);
      }
    }
    switch (patch.status) {
      case "completed":
      case "failed":
        events.push(...this.end(message.task_id, patch.status, patch.error ?? null));
        break;
      case "killed":
        events.push(...this.end(message.task_id, "stopped", patch.error ?? null));
        break;
      default:
        break;
    }
    return events;
  }

  private end(taskId: string, status: TaskStatus, summary: string | null): ChatEvent[] {
    const task = this.tasks.get(taskId);
    if (!task) return [];
    this.tasks.delete(taskId);
    if (!task.shown) return [];
    return [{ type: "task_ended", task_id: taskId, status, summary: summary ? cut(summary) : null }];
  }
}

function taskKind(type: string | undefined): TaskKind {
  switch (type) {
    case "local_bash":
      return "command";
    case "local_agent":
    case "remote_agent":
    case "in_process_teammate":
      return "subagent";
    default:
      return "other";
  }
}

function finite(value: number | undefined): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function cut(text: string): string {
  return text.length <= SUMMARY_LIMIT ? text : `${text.slice(0, SUMMARY_LIMIT)}…`;
}
