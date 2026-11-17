// What the foot of the sidebar says about a newer Chauffeur (`update.status`): nothing when none
// is published or the user hid this version, otherwise a card that follows the installation and
// the restart the user planned.

import type { PowerState, UpdateStatus } from "@chauffeur/protocol";
import { powerReason } from "./power";

export type UpdateCard =
  | { kind: "available"; version: string }
  | { kind: "installing"; version: string; step: string }
  | { kind: "installed"; version: string }
  | { kind: "failed"; version: string; error: string };

/** Where the version the user hid is kept, until a newer one is published. */
export const DISMISSED_KEY = "chauffeur.update.dismissed";

export function updateCard(status: UpdateStatus | null, dismissed: string | null): UpdateCard | null {
  const version = status?.available;
  if (!status || !version) return null;
  switch (status.phase) {
    case "downloading":
      return { kind: "installing", version, step: "Downloading…" };
    case "installing":
      return { kind: "installing", version, step: "Installing…" };
    case "installed":
      return { kind: "installed", version };
    case "failed":
      return dismissed === version ? null : { kind: "failed", version, error: status.error ?? "unknown error" };
    case "idle":
      return dismissed === version ? null : { kind: "available", version };
  }
}

/** Sent to the window once the menu's Check for Updates… found a version: the card shows it again. */
export const UPDATE_REVEAL_EVENT = "chauffeur:update-reveal";

/** What Check for Updates… says when nothing newer is published (an update under way still counts as one). */
export function upToDate(status: UpdateStatus): string | null {
  return status.available ? null : `Chauffeur ${status.current} is up to date.`;
}

/** Whether an update is under way: the card then follows it every second. */
export function updating(status: UpdateStatus | null): boolean {
  return status?.phase === "downloading" || status?.phase === "installing";
}

/** What to tell before restarting onto the new version, when that would interrupt agents. */
export function restartWarning(power: PowerState): string | null {
  if (idleForRestart(power)) return null;
  const reason = powerReason(power);
  return `${reason[0].toUpperCase()}${reason.slice(1)}: restarting interrupts their turns, which resume once Chauffeur is back.`;
}

/** Whether restarting now would interrupt nothing: no agent's turn, no workspace being created. */
export function idleForRestart(power: PowerState): boolean {
  return power.working_chats === 0 && power.creating_workspaces === 0;
}

/** What the card says while it waits to restart until no agent works. */
export function waitingForIdle(power: PowerState | null): string {
  if (!power || idleForRestart(power)) return "Restarts once no agent works.";
  return `Restarts once no agent works: ${powerReason(power)}.`;
}

/** The restart the user chose with the update: at once, or once no agent works. */
export type RestartPlan = "now" | "idle";

/** What the card says while the update downloads and installs, the restart planned after it. */
export function plannedRestart(plan: RestartPlan): string {
  return plan === "now" ? "Restarts once installed." : "Restarts once installed and no agent works.";
}
