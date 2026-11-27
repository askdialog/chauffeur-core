// The `/` menu of a composer, shared by the chat's and the creation dialog's first prompt.

import type { SlashCommand } from "@chauffeur/protocol";
import { useEffect, useMemo, useState } from "react";
import { matchCommands, slashQuery } from "@/lib/commands";
import { stepMatch } from "@/lib/chatFind";
import { menuKey } from "@/lib/composerMenu";
import { type CommandsSource, commandsKey, loadCommands, useApp } from "@/state/app";
import { Spinner } from "@/components/ui";
import { ComposerMenu } from "./ComposerMenu";

/** What the menu reads of a key press: a DOM event or React's. */
type SlashKeyEvent = Pick<
  KeyboardEvent,
  "key" | "shiftKey" | "ctrlKey" | "metaKey" | "altKey" | "isComposing" | "preventDefault"
>;

/**
 * The `/` menu of a composer: open while the message is a lone `/word`, it lists the agent's
 * commands matching the word (Claude's commands and skills, Codex's skills). ↑↓ move, ↵ or Tab
 * (without Shift: ⇧Tab opens the saved prompts by default) insert the command, Esc closes it.
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
    const action = menuKey(e);
    if (action === "close") {
      e.preventDefault();
      setDismissed(text);
      return true;
    }
    if (!action || matches.length === 0) return false;
    e.preventDefault();
    if (action === "pick") {
      const command = matches[Math.min(active, matches.length - 1)];
      if (command) pick(command);
    } else setActive((i) => stepMatch(i, matches.length, action === "down" ? 1 : -1));
    return true;
  };

  let message: React.ReactNode = null;
  if (list?.loading ?? true) {
    message = (
      <>
        <Spinner /> Listing the agent's commands…
      </>
    );
  } else if (list?.error) {
    message = `Could not list the commands: ${list.error}`;
  } else {
    message = `No command matches /${query ?? ""}`;
  }

  const menu = open ? (
    <ComposerMenu
      testId="slash-menu"
      items={matches}
      keyOf={(command) => command.name}
      active={active}
      message={message}
      onPick={pick}
      onHover={setActive}
    >
      {(command) => (
        <>
          <span className="shrink-0 font-medium text-fg">/{command.name}</span>
          {command.argument_hint && (
            <span className="max-w-[35%] shrink-0 truncate font-mono text-[11px] text-faint">{command.argument_hint}</span>
          )}
          <span className="min-w-0 truncate text-[12px] text-muted">{command.description}</span>
        </>
      )}
    </ComposerMenu>
  ) : null;
  /** Closes the menu until the text changes (the saved prompts opening in its place). */
  const close = () => {
    if (open) setDismissed(text);
  };
  return { menu, onKeyDown, close };
}
