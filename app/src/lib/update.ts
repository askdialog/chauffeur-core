// What the foot of the sidebar says about a newer Chauffeur (`update.status`): nothing when none
// is published or the user hid this version, otherwise a card that follows the installation.

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

/** Whether an update is under way: the card then follows it every second. */
export function updating(status: UpdateStatus | null): boolean {
  return status?.phase === "downloading" || status?.phase === "installing";
}

/** What to tell before restarting onto the new version, when that would interrupt agents. */
export function restartWarning(power: PowerState): string | null {
  if (power.working_chats === 0 && power.creating_workspaces === 0) return null;
  const reason = powerReason(power);
  return `${reason[0].toUpperCase()}${reason.slice(1)}: restarting interrupts their turns, which resume once Chauffeur is back.`;
}
