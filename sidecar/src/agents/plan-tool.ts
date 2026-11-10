// Claude's `update_plan` tool: the agent keeps its plan for multi-step work, the whole list each
// time a step starts or ends. Claude Code in Chauffeur has no task list of its own; Codex has its
// native plan and does not get this tool. The call only records the plan: the daemon reads it from
// the tool call (plan_progress.rs) into the chat's progress, which the user sees in the Office.

export const UPDATE_PLAN = "update_plan";

export const PLAN_STATUSES = ["pending", "in_progress", "completed"] as const;

export const UPDATE_PLAN_DESCRIPTION =
  "Keep your plan for work of several steps, so that the user sees how far you are: call it with the whole list of " +
  "steps and each one's status when you have planned the work, then again each time a step starts or is done. At most " +
  "one step in_progress at a time. Not for a task of one step or a quick answer.";

export const UPDATE_PLAN_STEPS = "Every step of the plan, in order, each with a short text and its status.";

export function recordPlan(input: unknown): { text: string; error: boolean } {
  const steps = (input as { steps?: unknown } | null)?.steps;
  if (!Array.isArray(steps) || steps.length === 0) return { text: "Give the plan's steps in `steps`.", error: true };
  const done = steps.filter((step) => (step as { status?: unknown })?.status === "completed").length;
  return { text: `Plan recorded: ${done}/${steps.length} done.`, error: false };
}
