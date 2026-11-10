// The permission modes a chat can run in, as the composer's picker shows them.

import type { PermissionMode } from "@chauffeur/protocol";
import { FilePen, Hand, ListChecks, type LucideIcon, ShieldCheck, ShieldOff } from "lucide-react";

export interface PermissionModeInfo {
  value: PermissionMode;
  /** On the picker's button. */
  label: string;
  /** In the picker's menu. */
  name: string;
  description: string;
  icon: LucideIcon;
  /** Shown in a warning tint: the mode never asks. */
  risky: boolean;
}

const MODES: Record<PermissionMode, Omit<PermissionModeInfo, "value">> = {
  auto: { label: "Auto", name: "Auto", description: "Asks only for risky actions", icon: ShieldCheck, risky: false },
  default: { label: "Ask", name: "Ask for permissions", description: "Asks before edits and commands", icon: Hand, risky: false },
  accept_edits: { label: "Accept edits", name: "Accept edits", description: "Edits freely, asks before commands", icon: FilePen, risky: false },
  plan: { label: "Plan", name: "Plan mode", description: "Reads and plans, changes nothing", icon: ListChecks, risky: false },
  bypass_permissions: { label: "Bypass", name: "Bypass permissions", description: "Never asks; use with care", icon: ShieldOff, risky: true },
};

/** In the picker's order: from the default (auto) to the mode that never asks. */
export const PERMISSION_MODES: PermissionModeInfo[] = (
  ["auto", "default", "accept_edits", "plan", "bypass_permissions"] as const
).map((value) => ({ value, ...MODES[value] }));

/** The mode new chats start in, as the daemon picks it. */
export const DEFAULT_PERMISSION_MODE: PermissionMode = "auto";

export function permissionMode(value: PermissionMode): PermissionModeInfo {
  return PERMISSION_MODES.find((mode) => mode.value === value) ?? { value, ...MODES.default, label: value, name: value };
}

/** What the picker's button shows: the mode the agent runs in, which may not be the one asked. */
export interface ShownPermissionMode {
  mode: PermissionModeInfo;
  /** Beside the label when the agent fell back from the mode asked. */
  hint: string | null;
  /** The button's tooltip, when it falls back. */
  tooltip: string | null;
}

/**
 * Claude Code falls back from auto to asking where auto is unavailable (the model, the plan or a
 * setting turns it off): the button then says so, while the menu keeps the mode asked checked.
 */
export function shownPermissionMode(
  requested: PermissionMode,
  effective: PermissionMode | null | undefined,
  model?: string | null,
): ShownPermissionMode {
  if (requested === "auto" && effective === "default") {
    const withModel = model ? ` with ${model}` : "";
    return {
      mode: permissionMode("default"),
      hint: "(Auto unavailable)",
      tooltip: `Auto mode is unavailable for this chat${withModel}: its model, plan or settings turn it off, so Claude asks before acting. It switches to auto again once Claude Code allows it, after a change of model for one.`,
    };
  }
  return { mode: permissionMode(requested), hint: null, tooltip: null };
}
