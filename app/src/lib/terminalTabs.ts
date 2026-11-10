// A terminal tab's name and its right-click menu, as chat tabs have: rename, close, close the
// others, open its shell's folder in the user's terminal app, copy that folder's path.

import type { OpenApp, TerminalInfo } from "@chauffeur/protocol";

/** What a tab shows: the name the user gave it, else its title, else its place. */
export function terminalLabel(terminal: TerminalInfo, index: number): string {
  return terminal.name || terminal.title || `Terminal ${index + 1}`;
}

/** The terminal app a terminal's folder opens in: the default one when it is a terminal, else the first found. */
export function terminalApp(apps: OpenApp[]): OpenApp | null {
  const terminals = apps.filter((app) => app.kind === "terminal");
  return terminals.find((app) => app.is_default) ?? terminals[0] ?? null;
}

export type TerminalAction = "rename" | "close" | "close-others" | "open" | "copy-path";

export type TerminalMenuItem =
  | { action: TerminalAction; label: string; shortcut: string; disabled?: boolean; hint?: string }
  | "separator";

/**
 * The menu of `terminal`'s tab among `terminals`. `apps` is null while the apps are looked for, `cwd`
 * while the shell's folder is read.
 */
export function terminalMenu({
  terminal,
  terminals,
  apps,
  cwd,
}: {
  terminal: TerminalInfo;
  terminals: TerminalInfo[];
  apps: OpenApp[] | null;
  cwd: string | null;
}): TerminalMenuItem[] {
  const app = apps ? terminalApp(apps) : null;
  const others = terminals.filter((t) => t.id !== terminal.id).length;
  return [
    { action: "rename", label: "Rename", shortcut: "r", hint: "Or double-click the tab; empty gives back its title" },
    { action: "close", label: "Close", shortcut: "c", hint: "Stops its shell" },
    {
      action: "close-others",
      label: "Close the others",
      shortcut: "o",
      disabled: others === 0,
      hint: others === 0 ? "It is the only terminal" : `Stops the shell of ${others === 1 ? "the other terminal" : `the ${others} others`}`,
    },
    "separator",
    {
      action: "open",
      label: app ? `Open in ${app.name}` : "Open in a terminal app",
      shortcut: "t",
      disabled: app == null,
      hint: apps == null ? "Looking for apps…" : app ? `Its folder: ${cwd ?? "…"}` : "No terminal app found on this machine",
    },
    {
      action: "copy-path",
      label: "Copy the path",
      shortcut: "p",
      hint: cwd ?? "Reading its folder…",
    },
  ];
}
