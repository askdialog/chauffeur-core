// A design chat's screen in its sandboxed iframe (story 47), on the canvas or interactive.

import type { CanvasScreen } from "@chauffeur/protocol";
import { useCallback } from "react";
import { screenSrc } from "@/lib/canvas";
import { withCapture } from "@/lib/comments";
import { cx } from "../ui";

/**
 * A screen in its sandboxed iframe at its own size: an opaque origin (no allow-same-origin), no
 * referrer, so that nothing of the app reaches it. A missing file is a card in its place.
 */
export function ScreenFrame({
  screen,
  scale = 1,
  interactive,
  capture,
  onFrame,
}: {
  screen: CanvasScreen;
  scale?: number;
  interactive: boolean;
  /** On the canvas: the daemon appends its capture script, for comments' pins (story 51). */
  capture?: boolean;
  /** The frame once mounted (null when gone) and again each time it loaded. */
  onFrame?: (frame: HTMLIFrameElement | null, loaded: boolean) => void;
}) {
  const track = useCallback((frame: HTMLIFrameElement | null) => onFrame?.(frame, false), [onFrame]);
  const raw = screenSrc(screen);
  const src = raw && capture ? withCapture(raw) : raw;
  if (!src) {
    return (
      <div
        className="flex h-full w-full items-center justify-center border border-dashed border-line-strong bg-panel p-[6%] text-center text-faint"
        style={{ width: screen.width, height: screen.height, fontSize: Math.min(14 / scale, screen.width / 9) }}
        data-testid="canvas-missing"
      >
        <span className="break-all">{screen.file} is missing</span>
      </div>
    );
  }
  return (
    <iframe
      ref={track}
      onLoad={(event) => onFrame?.(event.currentTarget, true)}
      src={src}
      title={screen.label || screen.file}
      sandbox="allow-scripts allow-forms"
      referrerPolicy="no-referrer"
      tabIndex={interactive ? 0 : -1}
      width={screen.width}
      height={screen.height}
      className={cx("block border-0 bg-white", !interactive && "pointer-events-none")}
      style={{ width: screen.width, height: screen.height }}
    />
  );
}

