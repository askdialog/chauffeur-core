// The saved prompts' menu of a chat's composer (docs/adr/saved-prompts.md): ⇧Tab opens it above the
// composer, its filter field takes the keys typed meanwhile, and ↵ hands the chosen prompt's text
// to the composer, never sending it.

import type { Prompt } from "@chauffeur/protocol";
import { useEffect, useRef, useState } from "react";
import { hasMod } from "@/lib/keys";
import { matchPrompts } from "@/lib/prompts";
import { errorMessage } from "@/lib/rpc";
import { client, openSettings } from "@/state/app";
import { Spinner } from "../ui";
import { ComposerMenu, stepActive } from "./ComposerMenu";

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
  // The latest opening: a list read for an earlier one is not shown.
  const opening = useRef(0);
  const latestLeave = useRef(leave);
  latestLeave.current = leave;

  const show = () => {
    const id = ++opening.current;
    setOpen(true);
    setPrompts(null);
    setError(null);
    setFilter("");
    setActive(0);
    client
      .call("prompts.list", {})
      .then((list) => {
        if (id === opening.current) setPrompts(list);
      })
      .catch((failure: unknown) => {
        if (id === opening.current) setError(errorMessage(failure));
      });
  };
  const hide = () => {
    opening.current += 1;
    setOpen(false);
  };
  const close = () => {
    hide();
    leave();
  };
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
      if (panel.current?.contains(event.target as Node)) return;
      opening.current += 1;
      setOpen(false);
      setTimeout(() => {
        const focus = document.activeElement;
        // The filter field, gone with the menu, is no text field any more.
        if (focus?.isConnected && focus.matches("input, textarea, select, [contenteditable=true]")) return;
        latestLeave.current();
      });
    };
    document.addEventListener("mousedown", onDown, true);
    return () => document.removeEventListener("mousedown", onDown, true);
  }, [open]);

  const matches = prompts ? matchPrompts(prompts, filter) : [];
  const onKeyDown = (event: React.KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === "Escape") {
      // Not for the chat too: it would interrupt the agent.
      event.preventDefault();
      event.stopPropagation();
      close();
    } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      if (matches.length > 0) setActive((i) => stepActive(i, step, matches.length));
    } else if ((event.key === "Tab" && !event.ctrlKey && !event.metaKey && !event.altKey) || (event.key === "Enter" && !event.shiftKey && !hasMod(event))) {
      // ⇧Tab included: the focus stays in the menu.
      event.preventDefault();
      if (event.key === "Tab" && event.shiftKey) return;
      if (prompts?.length === 0) return toSettings();
      const prompt = matches[Math.min(active, matches.length - 1)];
      if (prompt) pick(prompt);
    }
  };

  let message: React.ReactNode;
  if (error) message = `Could not read your prompts: ${error}`;
  else if (!prompts) {
    message = (
      <>
        <Spinner /> Reading your prompts…
      </>
    );
  } else if (prompts.length === 0) {
    message = (
      <span className="flex w-full items-center justify-between gap-2" data-testid="prompts-menu-empty">
        No prompt saved yet.
        <button
          type="button"
          className="rounded px-1.5 py-0.5 text-[12px] text-accent hover:bg-hover"
          onClick={toSettings}
          data-testid="prompts-menu-settings"
        >
          Open Settings › Prompts
        </button>
      </span>
    );
  } else message = `No prompt matches "${filter}"`;

  const menu = open ? (
    <ComposerMenu
      ref={panel}
      testId="prompts-menu"
      header={
        <input
          className="w-full shrink-0 border-b border-line bg-transparent px-3 py-1.5 text-[12.5px] text-fg outline-none placeholder:text-faint"
          placeholder="Filter your prompts…"
          aria-label="Filter your prompts"
          value={filter}
          onChange={(event) => {
            setFilter(event.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
          autoFocus
          data-testid="prompts-filter"
        />
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
