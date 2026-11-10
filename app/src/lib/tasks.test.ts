import { describe, expect, test } from "bun:test";
import type { TaskView } from "./chat";
import { foldedSummary, nestedCount, nestTasks, type TaskTree, taskLines, tasksHeader } from "./tasks";

function task(taskId: string, parentTaskId: string | null = null, kind: TaskView["kind"] = "subagent"): TaskView {
  return {
    taskId,
    kind,
    toolUseId: `tu-${taskId}`,
    description: `Task ${taskId}`,
    subagentType: kind === "subagent" ? "general-purpose" : null,
    background: true,
    parentTaskId,
    startedAt: "2026-10-18T10:00:00Z",
    lastTool: null,
    activity: null,
    summary: null,
    toolUses: null,
  };
}

/** The lines as `id@depth`. */
function lines(trees: TaskTree[]): string[] {
  return taskLines(trees).map((line) => `${line.task.taskId}@${line.depth}`);
}

describe("nestTasks", () => {
  test("a subagent's own tasks sit under it, the rest at the top", () => {
    const trees = nestTasks([task("a"), task("cmd", null, "command"), task("a-sleep", "a", "command")]);
    expect(trees.map((tree) => tree.task.taskId)).toEqual(["a", "cmd"]);
    expect(lines(trees)).toEqual(["a@0", "a-sleep@1", "cmd@0"]);
  });

  test("nesting goes as deep as the subagents do", () => {
    const trees = nestTasks([task("a"), task("b", "a"), task("c", "b"), task("d", "c", "command"), task("e", "d", "command")]);
    expect(lines(trees)).toEqual(["a@0", "b@1", "c@2", "d@3", "e@4"]);
  });

  test("a task whose parent is unknown or has ended stays at the top level", () => {
    const trees = nestTasks([task("x", "gone", "command"), task("a"), task("y", "a")]);
    expect(lines(trees)).toEqual(["x@0", "a@0", "y@1"]);
  });

  test("children come in start order under their parent, whatever came between", () => {
    const trees = nestTasks([task("a"), task("b"), task("a1", "a"), task("b1", "b"), task("a2", "a"), task("a1x", "a1", "command")]);
    expect(lines(trees)).toEqual(["a@0", "a1@1", "a1x@2", "a2@1", "b@0", "b1@1"]);
  });

  test("a child listed before its parent still nests under it", () => {
    expect(lines(nestTasks([task("c", "p", "command"), task("p")]))).toEqual(["p@0", "c@1"]);
  });

  test("no task is lost, even when parents point at each other", () => {
    const trees = nestTasks([task("a", "b"), task("b", "a"), task("self", "self")]);
    expect(lines(trees)).toEqual(["self@0", "a@0", "b@1"]);
  });

  test("no tasks, no trees", () => {
    expect(nestTasks([])).toEqual([]);
  });
});

describe("counts", () => {
  const trees = nestTasks([task("a"), task("a1", "a", "command"), task("a2", "a"), task("a2x", "a2", "command"), task("cmd", null, "command")]);

  test("nestedCount counts every task under a tree, at any depth", () => {
    expect(trees.map(nestedCount)).toEqual([3, 0]);
  });

  test("the folded line lists the top-level tasks with how many run under each", () => {
    expect(foldedSummary(trees)).toBe("Task a (+3) · Task cmd");
  });

  test("the header counts every task, and says how many sit under a subagent", () => {
    expect(tasksHeader(trees)).toEqual({ label: "5 tasks running", title: "3 of them started by a subagent, shown under it" });
    expect(tasksHeader(nestTasks([task("a"), task("b", "a")]))).toEqual({
      label: "2 tasks running",
      title: "1 of them started by a subagent, shown under it",
    });
    expect(tasksHeader(nestTasks([task("a")]))).toEqual({ label: "1 task running", title: null });
  });
});
