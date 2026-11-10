// A right-click menu, styled like the model picker: items with an icon and a one-key shortcut
// (pressed while the menu is open), separators, and submenus that open on hover.

import { ChevronRight } from "lucide-react";
import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { cx } from "./ui";

export type MenuItem =
  | {
      label: string;
      icon?: ReactNode;
      /** One letter, pressed while the menu is open. */
      shortcut?: string;
      danger?: boolean;
      /** Shown greyed, doing nothing; `hint` says why. */
      disabled?: boolean;
      /** A tooltip: what it does, or why it is disabled. */
      hint?: string;
      onSelect?: () => void;
      /** Opens on hover; the item itself does nothing. */
      submenu?: MenuItem[];
    }
  | "separator";

const WIDTH = 220;

export function ContextMenu({
  x,
  y,
  items,
  onClose,
}: {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number }>({ left: x, top: y });

  // Kept inside the window.
  useLayoutEffect(() => {
    const height = panel.current?.offsetHeight ?? 0;
    setPosition({
      left: Math.max(8, Math.min(x, window.innerWidth - WIDTH - 8)),
      top: Math.max(8, Math.min(y, window.innerHeight - height - 8)),
    });
  }, [x, y]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const item = items.find(
        (i) => i !== "separator" && i.shortcut && i.shortcut.toLowerCase() === event.key.toLowerCase(),
      );
      if (item && item !== "separator" && item.onSelect && !item.disabled) {
        event.preventDefault();
        event.stopPropagation();
        onClose();
        item.onSelect();
      }
    };
    const onDown = (event: MouseEvent) => {
      if (!(event.target as Element).closest?.("[data-context-menu]")) onClose();
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
    };
  }, [items, onClose]);

  return createPortal(
    <div
      ref={panel}
      data-popover
      data-context-menu
      role="menu"
      style={{ left: position.left, top: position.top, width: WIDTH }}
      className="fixed z-[70] rounded-xl border border-line-strong bg-sidebar p-1.5 text-[13px] shadow-2xl shadow-black/70"
      onContextMenu={(event) => event.preventDefault()}
    >
      <MenuList items={items} onClose={onClose} />
    </div>,
    document.body,
  );
}

function MenuList({ items, onClose }: { items: MenuItem[]; onClose: () => void }) {
  const [open, setOpen] = useState<number | null>(null);
  return (
    <>
      {items.map((item, index) =>
        item === "separator" ? (
          <div key={`sep-${index}`} className="mx-1 my-1 h-px bg-line" />
        ) : (
          <MenuRow
            key={item.label}
            item={item}
            open={open === index}
            onHover={() => setOpen(item.submenu ? index : null)}
            onClose={onClose}
          />
        ),
      )}
    </>
  );
}

function MenuRow({
  item,
  open,
  onHover,
  onClose,
}: {
  item: Exclude<MenuItem, "separator">;
  open: boolean;
  onHover: () => void;
  onClose: () => void;
}) {
  const row = useRef<HTMLButtonElement>(null);
  return (
    <>
      <button
        ref={row}
        type="button"
        role="menuitem"
        onMouseEnter={onHover}
        aria-disabled={item.disabled || undefined}
        title={item.hint}
        onClick={() => {
          if (item.submenu || item.disabled) return;
          onClose();
          item.onSelect?.();
        }}
        className={cx(
          "flex h-8 w-full items-center gap-2.5 rounded-lg px-2 text-left",
          item.disabled ? "cursor-default opacity-40" : open ? "bg-hover" : "hover:bg-hover",
          item.danger ? "text-danger" : "text-fg",
        )}
      >
        <span className={cx("flex w-4 shrink-0 justify-center", item.danger ? "text-danger" : "text-muted")}>
          {item.icon}
        </span>
        <span className="min-w-0 flex-1 truncate">{item.label}</span>
        {item.shortcut && <span className="shrink-0 text-[11.5px] text-faint">{item.shortcut.toUpperCase()}</span>}
        {item.submenu && <ChevronRight size={13} className="shrink-0 text-faint" />}
      </button>
      {open && item.submenu && <Submenu anchor={row} items={item.submenu} onClose={onClose} />}
    </>
  );
}

function Submenu({
  anchor,
  items,
  onClose,
}: {
  anchor: React.RefObject<HTMLButtonElement | null>;
  items: MenuItem[];
  onClose: () => void;
}) {
  const [style, setStyle] = useState<React.CSSProperties>({ visibility: "hidden" });
  const panel = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const rect = anchor.current?.getBoundingClientRect();
    if (!rect) return;
    const height = panel.current?.offsetHeight ?? 0;
    const right = rect.right + 6 + WIDTH < window.innerWidth;
    setStyle({
      left: right ? rect.right + 6 : rect.left - WIDTH - 6,
      top: Math.max(8, Math.min(rect.top - 6, window.innerHeight - height - 8)),
      width: WIDTH,
    });
  }, [anchor]);
  return createPortal(
    <div
      ref={panel}
      data-popover
      data-context-menu
      role="menu"
      style={style}
      className="fixed z-[71] rounded-xl border border-line-strong bg-sidebar p-1.5 text-[13px] shadow-2xl shadow-black/70"
    >
      <MenuList items={items} onClose={onClose} />
    </div>,
    document.body,
  );
}
