// The `/` menu above the composer: the agent's commands matching what follows the slash.

import type { SlashCommand } from "@chauffeur/protocol";
import { useEffect, useRef } from "react";
import { cx, Spinner } from "../ui";

export function SlashMenu({
  query,
  commands,
  active,
  loading,
  error,
  onPick,
  onHover,
}: {
  query: string;
  commands: SlashCommand[];
  active: number;
  loading: boolean;
  error: string | null;
  onPick: (command: SlashCommand) => void;
  onHover: (index: number) => void;
}) {
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  let message: React.ReactNode = null;
  if (commands.length === 0) {
    if (loading) {
      message = (
        <>
          <Spinner /> Listing the agent's commands…
        </>
      );
    } else if (error) {
      message = `Could not list the commands: ${error}`;
    } else {
      message = `No command matches /${query}`;
    }
  }

  return (
    <div
      ref={list}
      data-popover
      role="listbox"
      data-testid="slash-menu"
      className="absolute right-0 bottom-full left-0 z-20 mb-1.5 max-h-[280px] overflow-y-auto rounded-lg border border-line-strong bg-raised py-1 shadow-xl"
    >
      {message ? (
        <div className="flex items-center gap-2 px-3 py-1.5 text-[12px] text-faint">{message}</div>
      ) : (
        commands.map((command, index) => (
          <button
            key={command.name}
            type="button"
            role="option"
            aria-selected={index === active}
            data-index={index}
            // Keep the focus in the composer.
            onMouseDown={(event) => {
              event.preventDefault();
              onPick(command);
            }}
            onMouseEnter={() => onHover(index)}
            className={cx(
              "flex w-full items-baseline gap-2 px-3 py-1.5 text-left text-[12.5px]",
              index === active ? "bg-hover" : "",
            )}
          >
            <span className="shrink-0 font-medium text-fg">/{command.name}</span>
            {command.argument_hint && (
              <span className="max-w-[35%] shrink-0 truncate font-mono text-[11px] text-faint">{command.argument_hint}</span>
            )}
            <span className="min-w-0 truncate text-[12px] text-muted">{command.description}</span>
          </button>
        ))
      )}
    </div>
  );
}
