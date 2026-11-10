// Comments on a design chat's screens (story 51): a pin's disc and the box of the bubble or thread
// open at it, drawn alike on the canvas and over a screen shown full screen or in Present. There,
// `ScreenComments` lays them over the screen's interactive frame, with a cover that takes the
// clicks in Comment mode: a click pins a comment at that point of what the user sees.

import type { ReactNode } from "react";
import { type Draft, type DrawnPin, popoverAnchor, type Thread } from "@/lib/comments";
import { cx } from "../ui";

/** What full screen and Present need to comment on their screens, from the canvas that opened them. */
export type ScreenCommenting = {
  commentMode: boolean;
  onToggleCommentMode: () => void;
  /** The pins to draw, by file. */
  pins: Record<string, DrawnPin[]>;
  draft: Draft | null;
  openThread: Thread | null;
  /** A screen's frame in full screen or Present, mounted (null when gone) or loaded: its pins are checked there too. */
  onFrame: (file: string, frame: HTMLIFrameElement | null, loaded: boolean) => void;
  /** A click in Comment mode, at a point (percent) of a screen or on its label; `frame` is the one the user sees. */
  onCommentAt: (file: string, x: number, y: number, whole: boolean, frame?: HTMLIFrameElement | null) => void;
  onOpenThread: (id: string) => void;
  /** Closes the bubble, else the thread, else leaves Comment mode. */
  onEscape: () => void;
  /** The bubble or thread shown at the draft's or open thread's pin. */
  popover: ReactNode;
};

export function PinButton({ pin, open, left, top, onOpen }: { pin: DrawnPin; open: boolean; left: number; top: number; onOpen: () => void }) {
  return (
    <button
      type="button"
      className={cx(
        "pointer-events-auto absolute flex h-5 min-w-5 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full px-1 text-[10.5px] font-semibold text-white shadow-md ring-2 ring-white",
        pin.stale ? "bg-faint" : "bg-accent",
        open && "scale-110",
      )}
      style={{ left, top }}
      title={`${pin.thread.comment.author.name}: ${pin.thread.comment.text}${pin.stale ? " (stale)" : ""}`}
      onClick={onOpen}
      data-testid="canvas-pin"
      data-stale={pin.stale ? "" : undefined}
    >
      {pin.thread.number}
    </button>
  );
}

/** The bubble or thread beside its pin; the keys typed in it stay there, Esc still closes it. */
export function PinPopover({ left, top, onEscape, children }: { left: number; top: number; onEscape: () => void; children: ReactNode }) {
  return (
    <div
      className="pointer-events-auto absolute z-10"
      style={{ left, top }}
      data-canvas-popover
      onPointerDown={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        event.stopPropagation();
        if (event.key === "Escape") {
          event.preventDefault();
          onEscape();
        }
      }}
    >
      {children}
    </div>
  );
}

/**
 * The pins, the Comment mode cover and the open box over a screen `width` × `height` on screen;
 * `frame` finds its iframe, asked for the element under a new pin.
 */
export function ScreenComments({
  file,
  width,
  height,
  frame,
  commenting,
}: {
  file: string;
  width: number;
  height: number;
  frame: () => HTMLIFrameElement | null;
  commenting: ScreenCommenting;
}) {
  const { commentMode, pins, draft, openThread, popover, onCommentAt, onOpenThread, onEscape } = commenting;
  const anchor = popover ? popoverAnchor(file, width, height, draft, openThread) : null;
  return (
    <div className="pointer-events-none absolute inset-0" data-testid="screen-comments">
      {commentMode && (
        <div
          className="pointer-events-auto absolute inset-0 cursor-crosshair"
          title="Comment here"
          data-testid="screen-comment-cover"
          onClick={(event) => {
            const rect = event.currentTarget.getBoundingClientRect();
            const x = Math.min(100, Math.max(0, ((event.clientX - rect.left) / rect.width) * 100));
            const y = Math.min(100, Math.max(0, ((event.clientY - rect.top) / rect.height) * 100));
            onCommentAt(file, x, y, false, frame());
          }}
        />
      )}
      {(pins[file] ?? []).map((pin) => (
        <PinButton
          key={pin.thread.comment.id}
          pin={pin}
          open={openThread?.comment.id === pin.thread.comment.id}
          left={pin.whole ? 10 : (width * pin.x) / 100}
          top={pin.whole ? 10 : (height * pin.y) / 100}
          onOpen={() => onOpenThread(pin.thread.comment.id)}
        />
      ))}
      {anchor && (
        <PinPopover left={anchor.x + 12} top={anchor.y - 8} onEscape={onEscape}>
          {popover}
        </PinPopover>
      )}
    </div>
  );
}
