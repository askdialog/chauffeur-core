// A menu above the composer: the `/` menu's commands, the saved prompts' menu (⇧Tab). A click picks
// a row without taking the focus from where the menu's owner keeps it.

import { type ReactNode, type Ref, useEffect, useRef } from "react";
import { cx } from "../ui";

export function ComposerMenu<T>({
  ref,
  testId,
  header,
  items,
  keyOf,
  active,
  message,
  onPick,
  onHover,
  children: row,
}: {
  ref?: Ref<HTMLDivElement>;
  testId: string;
  /** Above the rows, out of their scrolling: a filter field. */
  header?: ReactNode;
  items: T[];
  keyOf: (item: T) => string;
  active: number;
  /** Shown instead of the rows when there is none: loading, an error, nothing matching. */
  message: ReactNode;
  onPick: (item: T) => void;
  onHover: (index: number) => void;
  children: (item: T) => ReactNode;
}) {
  const list = useRef<HTMLDivElement>(null);
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  return (
    <div
      ref={ref}
      data-popover
      data-testid={testId}
      className="absolute right-0 bottom-full left-0 z-20 mb-1.5 flex max-h-[280px] flex-col overflow-hidden rounded-lg border border-line-strong bg-raised shadow-xl"
    >
      {header}
      <div ref={list} role="listbox" className="min-h-0 overflow-y-auto py-1">
        {items.length === 0 ? (
          <div className="flex items-center gap-2 px-3 py-1.5 text-[12px] text-faint">{message}</div>
        ) : (
          items.map((item, index) => (
            <button
              key={keyOf(item)}
              type="button"
              role="option"
              aria-selected={index === active}
              data-index={index}
              // Keep the focus where it is.
              onMouseDown={(event) => {
                event.preventDefault();
                onPick(item);
              }}
              onMouseEnter={() => onHover(index)}
              className={cx(
                "flex w-full items-baseline gap-2 px-3 py-1.5 text-left text-[12.5px]",
                index === active ? "bg-hover" : "",
              )}
            >
              {row(item)}
            </button>
          ))
        )}
      </div>
    </div>
  );
}

/** The row ↓ (`step` 1) or ↑ (-1) moves to among `count`, wrapping around. */
export function stepActive(active: number, step: 1 | -1, count: number): number {
  return (active + step + count) % count;
}
