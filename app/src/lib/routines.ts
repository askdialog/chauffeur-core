// Routines (docs/adr/routines.md): how the sidebar and the Routine view word a routine's state
// and schedule. Pure: the daemon computes the next run, this only says it.

import type { RoutineFindInfo, RoutineInfo, RoutineSchedule, Weekday, WorkspaceInfo } from "@chauffeur/protocol";

const DAYS: Weekday[] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
const DAY_NAMES: Record<Weekday, string> = { mon: "Mon", tue: "Tue", wed: "Wed", thu: "Thu", fri: "Fri", sat: "Sat", sun: "Sun" };

/** The routine whose workspace this is, if any. */
export function routineOfWorkspace(routines: Record<string, RoutineInfo>, workspaceId: string): RoutineInfo | undefined {
  return Object.values(routines).find((routine) => routine.workspace_id === workspaceId);
}

/**
 * "Weekdays at 08:00", "Every day at 08:00 and 17:30", "Mon, Thu at 09:00", "Every 3 days at
 * 09:00"; "No schedule".
 */
export function scheduleText(schedule: RoutineSchedule): string {
  const days = DAYS.filter((day) => schedule.days.includes(day));
  const times = [...schedule.times].sort();
  const every = schedule.every_days;
  if ((days.length === 0 && !every) || times.length === 0) return "No schedule";
  const when = every
    ? every === 1
      ? "Every day"
      : `Every ${every} days`
    : days.length === 7
      ? "Every day"
      : days.length === 5 && !days.includes("sat") && !days.includes("sun")
        ? "Weekdays"
        : days.length === 2 && days.includes("sat") && days.includes("sun")
          ? "Weekends"
          : days.map((day) => DAY_NAMES[day]).join(", ");
  const at = times.length === 1 ? times[0] : `${times.slice(0, -1).join(", ")} and ${times[times.length - 1]}`;
  return `${when} at ${at}`;
}

/** A moment as the sidebar says it: "08:00" today, "Tue 08:00" within the week, else the date. */
export function whenText(rfc3339: string, now: Date): string {
  const date = new Date(rfc3339);
  const time = date.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
  const startOf = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOf(date) - startOf(now)) / 86_400_000);
  if (days === 0) return time;
  if (days > 0 && days < 7) return `${DAY_NAMES[DAYS[(date.getDay() + 6) % 7]]} ${time}`;
  return `${date.toLocaleDateString("en-GB", { day: "numeric", month: "short" })} ${time}`;
}

/** The routine's state in a few words, for its workspace's row and its view's header. */
export function stateText(routine: RoutineInfo, now: Date): string {
  if (routine.state === "draft") return "draft";
  if (routine.state === "paused") return "paused";
  if (routine.pending) return "active · run due";
  return routine.next_run_at ? `active · ${whenText(routine.next_run_at, now)}` : "active";
}

/**
 * The sidebar section the routines are filed in: the one most of their workspaces are in. None
 * before a routine is made, or once they were all taken out of it.
 */
export function routinesSection(routines: Record<string, RoutineInfo>, workspaces: Record<string, WorkspaceInfo>): string | null {
  const counts = new Map<string, number>();
  for (const routine of Object.values(routines)) {
    const section = workspaces[routine.workspace_id]?.section_id;
    if (section) counts.set(section, (counts.get(section) ?? 0) + 1);
  }
  let best: string | null = null;
  for (const [section, count] of counts) if (best == null || count > (counts.get(best) ?? 0)) best = section;
  return best;
}

/** What the sidebar's Suggestions entry says, or null when it has nothing to show. */
export function suggestionsText(find: RoutineFindInfo | null): string | null {
  if (!find) return null;
  if (find.running) return "Finding routines…";
  const waiting = find.suggestions.length;
  if (waiting > 0) return `Suggestions (${waiting})`;
  return null;
}
