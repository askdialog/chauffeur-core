// The `/` menu of a composer, shared by the chat's and the creation dialog's first prompt.

import type { SlashCommand } from "@chauffeur/protocol";
import { useEffect, useMemo, useState } from "react";
import { matchCommands, slashQuery } from "@/lib/commands";
import { hasMod } from "@/lib/keys";
import { type CommandsSource, commandsKey, loadCommands, useApp } from "@/state/app";
import { SlashMenu } from "./SlashMenu";

/** What the menu reads of a key press: a DOM event or React's. */
type SlashKeyEvent = Pick<
  KeyboardEvent,
  "key" | "shiftKey" | "ctrlKey" | "metaKey" | "altKey" | "isComposing" | "preventDefault"
>;

/**
 * The `/` menu of a composer: open while the message is a lone `/word`, it lists the agent's
 * commands matching the word (Claude's commands and skills, Codex's skills). ↑↓ move, ↵ or Tab
 * insert the command, Esc closes it.
 */
export function useSlashMenu(
  text: string,
  commandsFor: CommandsSource,
  insert: (command: SlashCommand) => void,
) {
  const query = slashQuery(text);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const open = query !== null && text !== dismissed;
  const key = commandsKey(commandsFor);
  const list = useApp((s) => s.commands[key]);
  const [active, setActive] = useState(0);

  useEffect(() => {
    if (open) void loadCommands(commandsFor);
    // The key says it all: `commandsFor` is a new object on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, key]);
  useEffect(() => setActive(0), [query]);

  const matches = useMemo(
    () => (open && query !== null ? matchCommands(list?.commands ?? [], query) : []),
    [open, list, query],
  );
  const pick = (command: SlashCommand) => {
    setDismissed(null);
    insert(command);
  };

  const onKeyDown = (e: SlashKeyEvent): boolean => {
    if (!open || e.isComposing) return false;
    if (e.key === "Escape") {
      e.preventDefault();
      setDismissed(text);
      return true;
    }
    if (matches.length === 0) return false;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const step = e.key === "ArrowDown" ? 1 : -1;
      setActive((i) => (i + step + matches.length) % matches.length);
      return true;
    }
    if ((e.key === "Tab" && !e.ctrlKey) || (e.key === "Enter" && !e.shiftKey && !hasMod(e))) {
      e.preventDefault();
      const command = matches[Math.min(active, matches.length - 1)];
      if (command) pick(command);
      return true;
    }
    return false;
  };

  const menu = open ? (
    <SlashMenu
      query={query ?? ""}
      commands={matches}
      active={active}
      loading={list?.loading ?? true}
      error={list?.error ?? null}
      onPick={pick}
      onHover={setActive}
    />
  ) : null;
  return { menu, onKeyDown };
}
