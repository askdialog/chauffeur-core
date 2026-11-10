// The agent's running tasks as the tasks bar shows them: a subagent's own tasks (a command it
// left in the background, its own subagents) nested under it, in start order.

import type { TaskView } from "./chat";

/** A running task and the running tasks it started, oldest first. */
export type TaskTree = { task: TaskView; children: TaskTree[] };

/** A line of the bar: a task and how deep it sits (0 at the top level). */
export type TaskLine = { task: TaskView; depth: number };

/** Indentation stops growing past this depth; deeper tasks line up with the third level. */
export const MAX_TASK_INDENT = 3;

/**
 * Nests `tasks` (oldest first) under the tasks that started them. A task whose parent is unknown,
 * or no longer running, stays at the top level, so none is lost.
 */
export function nestTasks(tasks: TaskView[]): TaskTree[] {
  const running = new Set(tasks.map((task) => task.taskId));
  const children = new Map<string, TaskView[]>();
  for (const task of tasks) {
    const parent = task.parentTaskId;
    if (parent === null || parent === task.taskId || !running.has(parent)) continue;
    children.set(parent, [...(children.get(parent) ?? []), task]);
  }
  const placed = new Set<string>();
  const grow = (task: TaskView): TaskTree => {
    placed.add(task.taskId);
    const below = (children.get(task.taskId) ?? []).filter((child) => !placed.has(child.taskId));
    return { task, children: below.map(grow) };
  };
  const isRoot = (task: TaskView) => task.parentTaskId === null || task.parentTaskId === task.taskId || !running.has(task.parentTaskId);
  const trees = tasks.filter(isRoot).map(grow);
  // Parents that point at each other (never sent, but nothing may disappear): the oldest leads.
  for (const task of tasks) if (!placed.has(task.taskId)) trees.push(grow(task));
  return trees;
}

/** The bar's lines: each task, then the tasks under it, depth first. */
export function taskLines(trees: TaskTree[], depth = 0): TaskLine[] {
  return trees.flatMap((tree) => [{ task: tree.task, depth }, ...taskLines(tree.children, depth + 1)]);
}

/** How many tasks run under `tree`, at any depth. */
export function nestedCount(tree: TaskTree): number {
  return tree.children.reduce((sum, child) => sum + 1 + nestedCount(child), 0);
}

/** The folded bar's line: the top-level tasks, each with how many run under it. */
export function foldedSummary(trees: TaskTree[]): string {
  return trees
    .map((tree) => {
      const under = nestedCount(tree);
      return under > 0 ? `${tree.task.description} (+${under})` : tree.task.description;
    })
    .join(" · ");
}

/** The bar's header: every running task counted (as the sidebar does), and how many sit under a subagent. */
export function tasksHeader(trees: TaskTree[]): { label: string; title: string | null } {
  const under = trees.reduce((sum, tree) => sum + nestedCount(tree), 0);
  const total = trees.length + under;
  const label = total === 1 ? "1 task running" : `${total} tasks running`;
  if (under === 0) return { label, title: null };
  return { label, title: `${under} of them started by a subagent, shown under it` };
}
