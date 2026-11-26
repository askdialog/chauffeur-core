// The saved prompts' menu of a chat's composer (docs/adr/saved-prompts.md): its shortcut,
// `prompts.open` (⇧Tab by default), opens it above the composer, its filter field takes the keys
// typed meanwhile, and ↵ hands the chosen prompt's text to the composer, never sending it.

import type { Prompt } from "@chauffeur/protocol";
import { Search } from "lucide-react";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { stepMatch } from "@/lib/chatFind";
import { menuKey } from "@/lib/composerMenu";
import { matchPrompts } from "@/lib/prompts";
import { pressed } from "@/lib/shortcuts";
import { errorMessage } from "@/lib/rpc";
import { client, openSettings } from "@/state/app";
import { Kbd, Spinner } from "@/components/ui";
import { ComposerMenu } from "./ComposerMenu";

/**
 * The prompts menu: `show` opens it and reads the prompts again. ↑↓ move, ↵ or Tab insert the
 * prompt (`insert`), Esc or a click outside close it (`leave`: the composer takes the focus back).
 */
export function usePromptsMenu(insert: (text: string) => void, leave: () => void) {
  const [open, setOpen] = useState(false);
  const [prompts, setPrompts] = useState<Prompt[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [active, setActive] = useState(0);
  const panel = useRef<HTMLDivElement>(null);
  const listId = useId();
  // The latest opening: a list read for an earlier one is not shown.
  const opening = useRef(0);
  const latestLeave = useRef(leave);
  latestLeave.current = leave;

  const load = () => {
    const id = ++opening.current;
    setPrompts(null);
    setError(null);
    client
      .call("prompts.list", {})
      .then((list) => {
        if (id === opening.current) setPrompts(list);
      })
      .catch((failure: unknown) => {
        if (id === opening.current) setError(errorMessage(failure));
      });
  };
  const show = () => {
    setOpen(true);
    setFilter("");
    setActive(0);
    load();
  };
  const hide = useCallback(() => {
    opening.current += 1;
    setOpen(false);
  }, []);
  const pick = (prompt: Prompt) => {
    hide();
    insert(prompt.text);
  };
  const toSettings = () => {
    hide();
    openSettings(true, "prompts");
  };

  // A click outside closes it, and the composer takes the focus back once the click gave it (to the
  // chat's scroller, a button…), unless the click went into another text field.
  useEffect(() => {
    if (!open) return;
    const onDown = (event: MouseEvent) => {
      if (event.target instanceof Node && panel.current?.contains(event.target)) return;
      hide();
      setTimeout(() => {
        const focus = document.activeElement;
        // The filter field, gone with the menu, is no text field any more.
        if (focus?.isConnected && focus.matches("input, textarea, select, [contenteditable=true]")) return;
        latestLeave.current();
      });
    };
    document.addEventListener("mousedown", onDown, true);
    return () => document.removeEventListener("mousedown", onDown, true);
  }, [open, hide]);

  const matches = prompts ? matchPrompts(prompts, filter) : [];
  const selected = Math.min(active, matches.length - 1);
  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    const { nativeEvent } = event;
    // The input method's key (WebKit may end composition before its confirming key, keyCode 229);
    // nor for the chat, whose Esc would interrupt the agent.
    if (nativeEvent.isComposing || nativeEvent.keyCode === 229) return event.stopPropagation();
    // Taken already: a subagent's panel closing on Esc.
    if (nativeEvent.defaultPrevented) return;
    // The focus stays in the menu: on its key again, and on ⇧Tab, which would leave it.
    if (pressed(nativeEvent, "prompts.open") || (event.key === "Tab" && event.shiftKey)) return event.preventDefault();
    const action = menuKey(event);
    if (!action) return;
    event.preventDefault();
    if (action === "close") {
      // Nor for the chat: it would interrupt the agent.
      event.stopPropagation();
      hide();
      leave();
    } else if (action === "pick") {
      if (prompts?.length === 0) return toSettings();
      const prompt = matches[selected];
      if (prompt) pick(prompt);
    } else if (matches.length > 0) setActive((i) => stepMatch(i, matches.length, action === "down" ? 1 : -1));
  };

  let message: React.ReactNode;
  if (error) {
    message = (
      <span className="flex items-center gap-2" role="alert">
        <span className="selectable">Could not read your prompts: {error}</span>
        <button
          type="button"
          className="rounded px-1.5 py-0.5 text-[12px] text-accent hover:bg-hover"
          // Keep the focus in the filter field, which holds the menu's keys.
          onMouseDown={(event) => event.preventDefault()}
          onClick={load}
          data-testid="prompts-menu-retry"
        >
          Retry
        </button>
      </span>
    );
  } else if (!prompts) {
    message = (
      <>
        <Spinner /> Reading your prompts…
      </>
    );
  } else if (prompts.length === 0) {
    message = (
      <span className="flex items-center gap-2" data-testid="prompts-menu-empty">
        No prompt saved yet.
        <button
          type="button"
          className="flex items-center gap-1.5 rounded px-1.5 py-0.5 text-[12px] text-accent hover:bg-hover"
          onClick={toSettings}
          data-testid="prompts-menu-settings"
        >
          Open Settings › Prompts <Kbd>↵</Kbd>
        </button>
      </span>
    );
  } else message = `No prompt matches "${filter}"`;

  const menu = open ? (
    <ComposerMenu
      ref={panel}
      id={listId}
      testId="prompts-menu"
      header={
        <div className="flex h-[34px] shrink-0 items-center gap-[7px] border-b border-line px-3">
          <Search size={13} className="shrink-0 text-faint" />
          <input
            className="h-full min-w-0 flex-1 bg-transparent text-[12.5px] text-fg placeholder:text-faint focus:outline-none"
            placeholder="Filter your prompts…"
            aria-label="Filter your prompts"
            aria-controls={listId}
            aria-activedescendant={matches.length > 0 ? `${listId}-${selected}` : undefined}
            value={filter}
            onChange={(event) => {
              setFilter(event.target.value);
              setActive(0);
            }}
            onKeyDown={onKeyDown}
            autoFocus
            data-testid="prompts-filter"
          />
        </div>
      }
      items={matches}
      keyOf={(prompt) => prompt.name}
      active={active}
      message={message}
      onPick={pick}
      onHover={setActive}
    >
      {(prompt) => (
        <>
          <span className="max-w-[60%] shrink-0 truncate font-medium text-fg">{prompt.name}</span>
          <span className="min-w-0 truncate text-[12px] text-muted">{prompt.text}</span>
        </>
      )}
    </ComposerMenu>
  ) : null;
  return { menu, show };
}
