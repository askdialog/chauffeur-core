// What the sidebar says about the machine kept awake (`power.get`): nothing while it may sleep,
// else an icon and a tooltip naming the mode and why.

import type { PowerState } from "@chauffeur/protocol";

export type PowerHint = {
  kind: "awake" | "lid_closed" | "low_battery" | "warning";
  /** The tooltip. */
  text: string;
};

/** "2 agents working, 1 workspace being created". */
export function powerReason(state: PowerState): string {
  const parts: string[] = [];
  if (state.working_chats > 0) parts.push(`${state.working_chats} ${state.working_chats === 1 ? "agent" : "agents"} working`);
  if (state.running_tasks > 0) parts.push(`${state.running_tasks} background ${state.running_tasks === 1 ? "task" : "tasks"}`);
  if (state.creating_workspaces > 0)
    parts.push(`${state.creating_workspaces} ${state.creating_workspaces === 1 ? "workspace" : "workspaces"} being created`);
  if (parts.length === 0 && state.lingering_secs != null) {
    const minutes = Math.ceil(state.lingering_secs / 60);
    return `the agents just finished; ${minutes} more ${minutes === 1 ? "minute" : "minutes"} in case they go on`;
  }
  return parts.join(", ");
}

export function powerHint(state: PowerState | null, mac: boolean): PowerHint | null {
  if (!state) return null;
  const machine = mac ? "the Mac" : "the computer";
  const Machine = mac ? "The Mac" : "The computer";
  const why = powerReason(state);
  if (state.holding === "all_sleep") {
    return {
      kind: "lid_closed",
      text: `Sleep is off, even with the lid closed: ${why}. It comes back when they stop, or on battery below the threshold set in Settings.`,
    };
  }
  if (state.warning) return { kind: "warning", text: state.warning };
  if (state.holding === "idle_sleep" && state.low_battery) {
    const percent = state.battery_percent != null ? ` (${state.battery_percent}%)` : "";
    return {
      kind: "low_battery",
      text: `Battery low${percent}: closing the lid sleeps ${machine} again until it charges. ${Machine} is still kept from idle sleep: ${why}.`,
    };
  }
  if (state.holding === "idle_sleep") {
    return {
      kind: "awake",
      text: `Keeping ${machine} awake: ${why}. The display may sleep; closing the lid still sleeps it.`,
    };
  }
  return null;
}
