// A right-click menu, styled like the model picker: items with an icon and a one-key shortcut
// (pressed while the menu is open), separators, and submenus that open on hover.

import { Check, ChevronRight } from "lucide-react";
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
      current?: boolean;
      /** A tooltip: what it does, or why it is disabled. */
      hint?: string;
      onSelect?: () => void;
      /** Opens on hover; the item itself does nothing. */
      submenu?: MenuItem[];
    }
  | "separator";

const WIDTH = 220;
const ENABLED_ITEM = '[role="menuitem"]:not([aria-disabled="true"])';
const PANEL_STYLE = "fixed max-h-[calc(100dvh-16px)] overflow-y-auto rounded-xl border border-line-strong bg-sidebar p-1.5 text-[13px] shadow-2xl shadow-black/70";

export function ContextMenu({
  x,
  y,
  anchor,
  label,
  items,
  onClose,
}: {
  x: number;
  y: number;
  anchor?: HTMLElement;
  label?: string;
  items: MenuItem[];
  onClose: () => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ left: number; top: number }>({ left: x, top: y });

  // Kept inside the window.
  useLayoutEffect(() => {
    const height = panel.current?.offsetHeight ?? 0;
    const rect = anchor?.getBoundingClientRect();
    const left = rect ? rect.right - WIDTH : x;
    const top = rect ? (rect.bottom + height + 12 <= window.innerHeight ? rect.bottom + 4 : rect.top - height - 4) : y;
    setPosition({
      left: Math.max(8, Math.min(left, window.innerWidth - WIDTH - 8)),
      top: Math.max(8, Math.min(top, window.innerHeight - height - 8)),
    });
    if (anchor) panel.current?.querySelector<HTMLElement>(ENABLED_ITEM)?.focus({ preventScroll: true });
  }, [anchor, x, y]);

  const closeAndFocus = () => {
    onClose();
    anchor?.focus({ preventScroll: true });
  };

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" || (anchor && event.key === "Tab")) {
        closeAndFocus();
        if (event.key === "Tab") return;
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
        const activeMenu = document.activeElement?.closest("[data-context-menu]") ?? panel.current;
        const buttons = [...(activeMenu?.querySelectorAll<HTMLElement>(ENABLED_ITEM) ?? [])];
        if (!buttons.length) return;
        const index = buttons.findIndex((button) => button === document.activeElement);
        const next = event.key === "Home" ? 0 : event.key === "End" ? buttons.length - 1
          : event.key === "ArrowDown" ? (index + 1) % buttons.length
          : index > 0 ? index - 1 : buttons.length - 1;
        event.preventDefault();
        event.stopPropagation();
        buttons[next]?.focus();
        return;
      }
      const item = items.find(
        (i) => i !== "separator" && i.shortcut && i.shortcut.toLowerCase() === event.key.toLowerCase(),
      );
      if (item && item !== "separator" && item.onSelect && !item.disabled) {
        event.preventDefault();
        event.stopPropagation();
        closeAndFocus();
        item.onSelect();
      }
    };
    const onDown = (event: MouseEvent) => {
      const target = event.target as Element;
      if (!target.closest?.("[data-context-menu]") && !anchor?.contains(target)) onClose();
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
  }, [anchor, items, onClose]);

  return createPortal(
    <div
      ref={panel}
      data-popover
      data-context-menu
      role="menu"
      aria-label={label}
      style={{ left: position.left, top: position.top, width: WIDTH }}
      className={cx(PANEL_STYLE, "z-[70]")}
      onContextMenu={(event) => event.preventDefault()}
    >
      <MenuList items={items} onClose={closeAndFocus} />
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
            onBack={() => setOpen(null)}
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
  onBack,
  onClose,
}: {
  item: Exclude<MenuItem, "separator">;
  open: boolean;
  onHover: () => void;
  onBack: () => void;
  onClose: () => void;
}) {
  const row = useRef<HTMLButtonElement>(null);
  const submenu = useRef<HTMLDivElement>(null);
  const openSubmenu = () => {
    onHover();
    requestAnimationFrame(() => submenu.current?.querySelector<HTMLElement>(ENABLED_ITEM)?.focus());
  };
  return (
    <>
      <button
        ref={row}
        type="button"
        role="menuitem"
        onMouseEnter={onHover}
        onFocus={onBack}
        aria-disabled={item.disabled || undefined}
        aria-current={item.current || undefined}
        aria-haspopup={item.submenu ? "menu" : undefined}
        aria-expanded={item.submenu ? open : undefined}
        title={item.hint}
        onKeyDown={(event) => {
          if (event.key !== "ArrowRight" || !item.submenu || item.disabled) return;
          event.preventDefault();
          openSubmenu();
        }}
        onClick={() => {
          if (item.disabled) return;
          if (item.submenu) {
            openSubmenu();
            return;
          }
          onClose();
          item.onSelect?.();
        }}
        className={cx(
          "flex h-8 w-full items-center gap-2.5 rounded-lg px-2 text-left",
          item.disabled ? "cursor-default opacity-50" : open ? "bg-hover" : "hover:bg-hover focus-visible:bg-hover",
          item.danger ? "text-danger" : "text-fg",
        )}
      >
        <span className={cx("flex w-4 shrink-0 justify-center", item.danger ? "text-danger" : "text-muted")}>
          {item.icon}
        </span>
        <span className="min-w-0 flex-1 truncate">{item.label}</span>
        {item.shortcut && <span className="shrink-0 text-[11.5px] text-faint">{item.shortcut.toUpperCase()}</span>}
        {item.current && <Check size={13} className="shrink-0 text-faint" />}
        {item.submenu && <ChevronRight size={13} className="shrink-0 text-faint" />}
      </button>
      {open && item.submenu && (
        <Submenu anchor={row} panel={submenu} items={item.submenu} onClose={onClose} onBack={onBack} />
      )}
    </>
  );
}

function Submenu({
  anchor,
  panel,
  items,
  onClose,
  onBack,
}: {
  anchor: React.RefObject<HTMLButtonElement | null>;
  panel: React.RefObject<HTMLDivElement | null>;
  items: MenuItem[];
  onClose: () => void;
  onBack: () => void;
}) {
  const [style, setStyle] = useState<React.CSSProperties>({ visibility: "hidden" });
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
  }, [anchor, panel]);
  return createPortal(
    <div
      ref={panel}
      data-popover
      data-context-menu
      role="menu"
      style={style}
      className={cx(PANEL_STYLE, "z-[71]")}
      onKeyDown={(event) => {
        if (event.key !== "ArrowLeft") return;
        event.preventDefault();
        event.stopPropagation();
        onBack();
        anchor.current?.focus();
      }}
    >
      <MenuList items={items} onClose={onClose} />
    </div>,
    document.body,
  );
}
