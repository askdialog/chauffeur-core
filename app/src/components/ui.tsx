import type { Activity, AgentKind, WorkspaceInfo } from "@chauffeur/protocol";
import { Check, ChevronDown, Loader2, Lock, PenTool, X } from "lucide-react";
import { type ButtonHTMLAttributes, type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

export function cx(...classes: Array<string | false | null | undefined>): string {
  return classes.filter(Boolean).join(" ");
}

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: "primary" | "secondary" | "ghost" | "danger" | "success";
  size?: "xs" | "sm" | "md";
  busy?: boolean;
};

export function Button({ variant = "secondary", size = "md", busy, className, children, disabled, ...rest }: ButtonProps) {
  return (
    <button
      {...rest}
      disabled={disabled || busy}
      className={cx(
        "inline-flex items-center justify-center font-medium whitespace-nowrap transition-colors disabled:opacity-50 disabled:pointer-events-none",
        size === "xs"
          ? "h-[18px] gap-0.5 rounded px-1.5 text-[11px]"
          : size === "sm"
            ? "h-6 gap-1.5 rounded-md px-2 text-[12px]"
            : "h-7 gap-1.5 rounded-md px-2.5 text-[12.5px]",
        variant === "primary" && "bg-accent-strong text-white hover:bg-accent",
        variant === "secondary" && "bg-raised text-fg border border-line hover:bg-hover hover:border-line-strong",
        variant === "ghost" && "text-muted hover:text-fg hover:bg-hover",
        variant === "danger" && "bg-danger/15 text-danger border border-danger/30 hover:bg-danger/25",
        variant === "success" && "bg-success-strong text-white hover:bg-success-hover",
        className,
      )}
    >
      {busy && <Loader2 size={12} className="animate-spin" />}
      {children}
    </button>
  );
}

export function IconButton({
  title,
  className,
  children,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { title: string }) {
  return (
    <button
      {...rest}
      title={title}
      aria-label={title}
      className={cx(
        "inline-flex h-6 w-6 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-fg disabled:opacity-40 disabled:pointer-events-none",
        className,
      )}
    >
      {children}
    </button>
  );
}

/** An on/off switch's look; the clickable element around it holds the state. */
export function Toggle({ on }: { on: boolean }) {
  return (
    <span
      className={cx(
        "relative inline-flex h-[18px] w-8 shrink-0 items-center rounded-full transition-colors",
        on ? "bg-accent-strong" : "bg-line-strong",
      )}
    >
      <span className={cx("absolute h-3.5 w-3.5 rounded-full bg-white shadow transition-transform", on ? "translate-x-4" : "translate-x-0.5")} />
    </span>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return (
    <kbd className="rounded border border-line bg-raised px-1 font-sans text-[10.5px] text-faint">{children}</kbd>
  );
}

export function Spinner({ size = 12, className }: { size?: number; className?: string }) {
  return <Loader2 size={size} className={cx("animate-spin", className)} />;
}

export function Badge({
  tone = "neutral",
  children,
  className,
  title,
}: {
  tone?: "neutral" | "accent" | "working" | "attention" | "danger" | "success";
  children: ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <span
      title={title}
      className={cx(
        "inline-flex items-center gap-1 rounded px-1.5 text-[11px] leading-[18px] font-medium whitespace-nowrap",
        tone === "neutral" && "bg-raised text-muted border border-line",
        tone === "accent" && "bg-accent/15 text-accent",
        tone === "working" && "bg-working/15 text-working",
        tone === "attention" && "bg-attention/15 text-attention",
        tone === "danger" && "bg-danger/15 text-danger",
        tone === "success" && "bg-success/15 text-success",
        className,
      )}
    >
      {children}
    </span>
  );
}

/** The dot shown next to a workspace: what its agents do, or its lifecycle state. */
export function StatusDot({ workspace }: { workspace: WorkspaceInfo }) {
  const { tone, pulse, label } = workspaceStatus(workspace);
  return (
    <span
      title={label}
      className={cx(
        "inline-block h-2 w-2 shrink-0 rounded-full",
        tone === "working" && "bg-working",
        tone === "attention" && "bg-attention",
        tone === "danger" && "bg-danger",
        tone === "idle" && "bg-faint/60",
        tone === "muted" && "border border-faint bg-transparent",
        pulse && "animate-pulse-dot",
      )}
    />
  );
}

export function workspaceStatus(workspace: WorkspaceInfo): {
  tone: "working" | "attention" | "danger" | "idle" | "muted";
  pulse: boolean;
  label: string;
} {
  switch (workspace.state) {
    case "creating":
      return { tone: "working", pulse: true, label: "Creating" };
    case "archiving":
      return { tone: "muted", pulse: true, label: "Archiving" };
    case "archived":
      return { tone: "muted", pulse: false, label: "Archived" };
    case "cleanup_failed":
      return { tone: "danger", pulse: false, label: "Cleanup failed" };
    case "ready":
      return activityStatus(workspace.activity);
  }
}

function activityStatus(activity: Activity) {
  switch (activity) {
    case "working":
      return { tone: "working" as const, pulse: true, label: "Working" };
    case "needs_input":
      return { tone: "attention" as const, pulse: false, label: "Needs input" };
    case "error":
      return { tone: "danger" as const, pulse: false, label: "Error" };
    case "idle":
      return { tone: "idle" as const, pulse: false, label: "Idle" };
  }
}

/** The open modals, the last on top: Escape closes only that one, not the dialog under it. */
const openModals: object[] = [];

export function Modal({
  open,
  onClose,
  title,
  children,
  footer,
  width = 560,
  height,
  dismissable = true,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
  footer?: ReactNode;
  width?: number;
  height?: number;
  dismissable?: boolean;
}) {
  const panel = useRef<HTMLDivElement>(null);
  // Read at the key press: a new `onClose` each render must not move this modal up the stack.
  const close = useRef({ onClose, dismissable });
  close.current = { onClose, dismissable };
  useEffect(() => {
    if (!open) return;
    const self = {};
    openModals.push(self);
    const onKey = (event: KeyboardEvent) => {
      // An open popover (the model picker) closes first.
      if (event.key !== "Escape" || openModals.at(-1) !== self || document.querySelector("[data-popover]")) return;
      event.stopPropagation();
      if (close.current.dismissable) close.current.onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => {
      openModals.splice(openModals.indexOf(self), 1);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [open]);
  if (!open) return null;
  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/55 pt-[10vh]"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget && dismissable) onClose();
      }}
    >
      <div
        ref={panel}
        role="dialog"
        aria-label={typeof title === "string" ? title : undefined}
        className="flex max-h-[80vh] flex-col overflow-hidden rounded-lg border border-line-strong bg-panel shadow-2xl shadow-black/60"
        style={{ width, height, maxWidth: "calc(100vw - 32px)" }}
      >
        <div className="flex h-10 shrink-0 items-center justify-between border-b border-line px-4">
          <div className="font-semibold text-fg">{title}</div>
          {dismissable && (
            <IconButton title="Close" onClick={onClose}>
              <X size={14} />
            </IconButton>
          )}
        </div>
        <div className="min-h-0 flex-1 overflow-y-auto p-4">{children}</div>
        {footer && <div className="flex shrink-0 items-center justify-end gap-2 border-t border-line px-4 py-2.5">{footer}</div>}
      </div>
    </div>
  );
}

export function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="block" role="group" aria-label={label}>
      <div className="mb-1 flex items-baseline justify-between">
        <span className="text-[12px] font-medium text-muted">{label}</span>
        {hint && <span className="text-[11px] text-faint">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

export const inputClass =
  "h-7 w-full rounded-md border border-line bg-bg px-2 text-[12.5px] text-fg placeholder:text-faint focus:border-accent/60 focus:outline-none";

export interface SelectOption<T extends string> {
  value: T;
  /** On the button, and in the menu unless `name` is given. */
  label: string;
  name?: string;
  /** A line under the name, in the menu. */
  description?: string;
  icon?: ReactNode;
  /** A warning tint, for a choice to make with care. */
  warning?: boolean;
  /** Why it cannot be picked now: shown in place of the description. */
  unavailable?: string | null;
}

/**
 * A choice among a few options: a borderless button that opens a menu above or below it, as the
 * model picker does. Arrows move, Enter picks, Escape closes.
 */
export function Select<T extends string>({
  value,
  options,
  onChange,
  className,
  title,
  disabled,
  testId,
  width = 240,
  shown,
}: {
  value: T;
  options: Array<SelectOption<T>>;
  onChange: (value: T) => void;
  className?: string;
  title?: string;
  disabled?: boolean;
  testId?: string;
  /** Of the menu. */
  width?: number;
  /** What the button shows instead of the option picked (a mode the agent fell back to). */
  shown?: { label: string; icon?: ReactNode; hint?: string | null; tooltip?: string };
}) {
  const [open, setOpen] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);
  const picked = options.find((option) => option.value === value);
  const current = shown ? { ...picked, ...shown } : picked;
  /** Back to the button, unless a click elsewhere took the focus. */
  const close = (refocus = true) => {
    setOpen(false);
    if (refocus) trigger.current?.focus();
  };
  return (
    <>
      <button
        ref={trigger}
        type="button"
        title={shown?.tooltip ?? title}
        aria-label={title}
        aria-haspopup="listbox"
        aria-expanded={open}
        disabled={disabled}
        data-testid={testId}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(event) => {
          if (!open && (event.key === "ArrowDown" || event.key === "ArrowUp")) {
            event.preventDefault();
            setOpen(true);
          }
        }}
        className={cx(
          "inline-flex h-6 max-w-[240px] items-center gap-1.5 rounded-md px-1.5 text-[12px] transition-colors disabled:opacity-50",
          open ? "bg-hover" : "hover:bg-hover",
          current?.warning ? "text-attention" : open ? "text-fg" : "text-muted hover:text-fg",
          className,
        )}
      >
        {current?.icon}
        <span className="truncate">{current?.label ?? value}</span>
        {shown?.hint && <span className="shrink-0 text-faint">{shown.hint}</span>}
        <ChevronDown size={12} className={cx("shrink-0", current?.warning ? "text-attention/70" : "text-faint")} />
      </button>
      {open && (
        <SelectMenu
          anchor={trigger}
          label={title}
          options={options}
          value={value}
          width={width}
          onPick={(picked) => {
            close();
            if (picked !== value) onChange(picked);
          }}
          onClose={close}
        />
      )}
    </>
  );
}

function SelectMenu<T extends string>({
  anchor,
  label,
  options,
  value,
  width,
  onPick,
  onClose,
}: {
  anchor: React.RefObject<HTMLButtonElement | null>;
  label: string | undefined;
  options: Array<SelectOption<T>>;
  value: T;
  width: number;
  onPick: (value: T) => void;
  onClose: (refocus?: boolean) => void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const [highlight, setHighlight] = useState(() => Math.max(0, options.findIndex((option) => option.value === value)));
  const described = options.some((option) => option.description || option.unavailable);
  const position = usePopoverPosition(anchor, width, options.length * (described ? 46 : 32) + 20);

  // Once placed: an element still hidden cannot take the focus.
  useEffect(() => {
    const frame = requestAnimationFrame(() => panel.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, []);

  // Clicks outside close the menu (the button toggles it itself).
  const latestClose = useRef(onClose);
  latestClose.current = onClose;
  useEffect(() => {
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (panel.current?.contains(target) || anchor.current?.contains(target)) return;
      latestClose.current(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [anchor]);

  // Escape closes the menu wherever the focus is; a Modal around it, or a chat, does not take it.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      latestClose.current();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      setHighlight((h) => (h + step + options.length) % options.length);
    } else if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      setHighlight(event.key === "Home" ? 0 : options.length - 1);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const option = options[highlight];
      if (option && !option.unavailable) onPick(option.value);
    } else if (event.key === "Tab") {
      event.preventDefault();
      onClose();
    }
  };

  return createPortal(
    <div
      ref={panel}
      role="listbox"
      aria-label={label}
      aria-activedescendant={`select-option-${highlight}`}
      tabIndex={-1}
      data-popover
      onKeyDown={onKeyDown}
      className="fixed z-[60] flex flex-col overflow-hidden rounded-xl border border-line-strong bg-sidebar p-1.5 text-[13px] shadow-2xl shadow-black/70 focus:outline-none"
      style={{ ...position, width }}
    >
      <div className="min-h-0 overflow-y-auto rounded-lg border border-line bg-raised py-1">
        {options.map((option, i) => {
          const selected = option.value === value;
          return (
            <div
              key={option.value}
              id={`select-option-${i}`}
              role="option"
              aria-selected={selected}
              aria-disabled={option.unavailable ? true : undefined}
              data-value={option.value}
              onMouseDown={(event) => event.preventDefault()}
              onMouseEnter={() => setHighlight(i)}
              onClick={() => !option.unavailable && onPick(option.value)}
              className={cx(
                "mx-1 flex cursor-default items-center gap-2.5 rounded-md px-2.5",
                option.description || option.unavailable ? "py-1.5" : "h-8",
                highlight === i && "bg-hover",
              )}
            >
              {option.icon && (
                <span
                  className={cx(
                    "flex w-4 shrink-0 justify-center",
                    option.unavailable ? "text-faint" : option.warning ? "text-attention" : "text-muted",
                  )}
                >
                  {option.icon}
                </span>
              )}
              <div className="min-w-0 flex-1">
                <div className={cx("truncate", option.unavailable ? "text-faint" : option.warning ? "text-attention" : "text-fg")}>
                  {option.name ?? option.label}
                </div>
                {option.unavailable ? (
                  <div className="text-[11.5px] text-faint">{option.unavailable}</div>
                ) : (
                  option.description && <div className="truncate text-[11.5px] text-faint">{option.description}</div>
                )}
              </div>
              {selected && <Check size={14} className="shrink-0 text-fg" />}
            </div>
          );
        })}
      </div>
    </div>,
    document.body,
  );
}

/**
 * A panel anchored under (or above) an element, outside the dialog's scrolling body: a click
 * outside it or Escape closes it, before any Modal under it. Its `data-autofocus` element takes
 * the focus once it is placed.
 */
export function Popover({
  anchor,
  width,
  height,
  onClose,
  children,
  className,
  testId,
}: {
  anchor: React.RefObject<HTMLElement | null>;
  width: number;
  /** The height it needs: below the anchor when that fits, above otherwise. */
  height: number;
  onClose: () => void;
  children: ReactNode;
  className?: string;
  testId?: string;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const position = usePopoverPosition(anchor, width, height);
  const latestClose = useRef(onClose);
  latestClose.current = onClose;
  // Once placed (an element still hidden cannot take the focus): its `data-autofocus` field.
  useEffect(() => {
    const frame = requestAnimationFrame(() => panel.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus());
    return () => cancelAnimationFrame(frame);
  }, []);
  useEffect(() => {
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node;
      if (panel.current?.contains(target) || anchor.current?.contains(target)) return;
      latestClose.current();
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      latestClose.current();
    };
    // Captured, before React handles the click: a row it then removes is still inside.
    document.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [anchor]);
  return createPortal(
    <div
      ref={panel}
      data-popover
      data-testid={testId}
      className={cx(
        "fixed z-[60] flex flex-col overflow-hidden rounded-lg border border-line-strong bg-raised text-[12.5px] shadow-2xl shadow-black/60",
        className,
      )}
      style={{ ...position, width }}
    >
      {children}
    </div>,
    document.body,
  );
}

/**
 * Fixed position for a popover next to its anchor: below it when `height` fits there (or there
 * is more room below), above otherwise, as in the composer.
 */
export function usePopoverPosition(
  anchor: React.RefObject<HTMLElement | null>,
  width: number,
  height: number,
): React.CSSProperties {
  const [style, setStyle] = useState<React.CSSProperties>({ visibility: "hidden" });
  useLayoutEffect(() => {
    const place = () => {
      const rect = anchor.current?.getBoundingClientRect();
      if (!rect) return;
      const left = Math.max(8, Math.min(rect.left - 6, window.innerWidth - width - 8));
      const below = window.innerHeight - rect.bottom - 14;
      const above = rect.top - 14;
      setStyle(
        below >= height || below >= above
          ? { left, top: rect.bottom + 6, maxHeight: below }
          : { left, bottom: window.innerHeight - rect.top + 6, maxHeight: above },
      );
    };
    place();
    window.addEventListener("resize", place);
    window.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", place, true);
    };
  }, [anchor, width, height]);
  return style;
}

/** Stands in for the permission mode picker in an exploration, whose chats cannot change anything. */
export function ReadOnlyMode() {
  return (
    <span
      title="An exploration's agents read and explain: they cannot edit, commit, push or open pull requests"
      className="inline-flex h-6 items-center gap-1 rounded-md border border-line bg-raised px-1.5 text-[12px] text-muted"
      data-testid="read-only-mode"
    >
      <Lock size={10} className="text-faint" /> Read-only
    </span>
  );
}

/** Stands in for the permission mode picker in a design chat (story 47), whose mode is fixed. */
export function DesignOnlyMode({ agent }: { agent: AgentKind }) {
  return (
    <span
      title={`${
        agent === "codex"
          ? "A design chat's Codex never asks to leave its sandbox"
          : "A design chat's agent runs in auto mode (accepting edits where auto is unavailable)"
      }: it reads the repos and the notes, and writes screens in its own design folder only, never the code`}
      className="inline-flex h-6 shrink-0 items-center gap-1 whitespace-nowrap rounded-md border border-line bg-raised px-1.5 text-[12px] text-muted"
      data-testid="design-only-mode"
    >
      <PenTool size={10} className="text-faint" /> Design only
    </span>
  );
}

export function EmptyState({ icon, title, children }: { icon?: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex h-full min-h-[160px] flex-col items-center justify-center gap-2 p-6 text-center">
      {icon && <div className="text-faint">{icon}</div>}
      <div className="font-medium text-muted">{title}</div>
      {children && <div className="max-w-md text-[12px] text-faint">{children}</div>}
    </div>
  );
}
