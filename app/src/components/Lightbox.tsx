// An image shown full screen, which zooms: pinch or ⌘ + wheel around the cursor, + − 0 on the
// keyboard, a double click between the fitted size and 100%; dragged or scrolled once zoomed.
// Esc, or a click beside the image, closes it. Opened from images shown together, ← → (or the
// side buttons) move to the previous or next one, shown fitted; they do so when zoomed in too: a
// drag or a scroll pans.
//
// Drawn so that it neither jerks nor shimmers in WebKit:
// - No CSS transition. Restarted by every wheel event, one froze the image, then made it leap; and
//   WebKit runs a transform transition on a layer of its own, a bitmap scaled until it ends.
// - Pinches, wheels and drags follow the hand: one update per frame. Only the discrete steps
//   (buttons, keys, double click) animate, frame by frame.
// - A plain 2D transform, no composited layer (translate3d, will-change): WebKit paints the
//   image (an SVG as vectors) at the zoom it is shown at, so it is sharp at any zoom. Laying the
//   image out at its zoomed size instead cost WebKit 0.1 to 0.7 s per change at large zooms.
// - Its corner sits on whole device pixels: at a new sub-pixel offset each frame, a pan would
//   draw its edges anew each time.

import { ChevronLeft, ChevronRight, Minus, Plus } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { between, FITTED, fitSize, followImage, type GalleryImage, galleryStep, pan, placement, type Size, wheelFactor, type Zoom, zoomAt } from "@/lib/zoom";
import { cx } from "./ui";

const STEP = 1.25;
type KeyAction = "close" | "in" | "out" | "fit" | "previous" | "next";
// The arrows are taken from whatever is underneath (a list, the composer) even for a lone image.
const KEYS: Record<string, KeyAction> = {
  Escape: "close",
  "+": "in",
  "=": "in",
  "-": "out",
  "0": "fit",
  ArrowLeft: "previous",
  ArrowRight: "next",
};
/** How long a discrete step animates. */
const STEP_MS = 120;

export type LightboxImage = GalleryImage;

/**
 * Shows `images[start]`; the others are a step away with the arrows. The set may change while it
 * is open (a composer's thumbnails): the image shown is followed, and an empty set closes it.
 */
export function Lightbox({ images, start = 0, onClose }: { images: LightboxImage[]; start?: number; onClose: () => void }) {
  const [at, setAt] = useState(() => ({ index: start, image: images[start] ?? { src: null, name: "" } }));
  const followed = followImage(images, at.image, at.index);
  const index = followed ?? 0;
  const { src, name } = images[index] ?? at.image;
  const count = images.length;
  /** The zoom shown now: during an animated step, a point on the way to `target`. */
  const [zoom, setZoom] = useState<Zoom>(FITTED);
  const shown = useRef<Zoom>(FITTED);
  const target = useRef<Zoom>(FITTED);
  /** The image's own size, once loaded. */
  const [natural, setNatural] = useState<Size | null>(null);
  // The set changed under it: the image now shown is the one followed from here on.
  if (followed !== null && (followed !== at.index || src !== at.image.src || name !== at.image.name)) {
    if (src !== at.image.src) setNatural(null);
    setAt({ index: followed, image: { src, name } });
  }
  useEffect(() => {
    if (followed === null) onClose();
  }, [followed, onClose]);
  /** The size of the area the image is fitted to. */
  const [box, setBox] = useState<Size | null>(null);
  const [dragging, setDragging] = useState(false);
  const area = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; y: number; moved: boolean } | null>(null);

  const animation = useRef<number | null>(null);
  /** Shows `next`: at once for a gesture, animated from the zoom shown for a discrete step. */
  const show = useCallback((next: Zoom, animate: boolean) => {
    if (animation.current != null) cancelAnimationFrame(animation.current);
    animation.current = null;
    target.current = next;
    const display = (value: Zoom) => {
      shown.current = value;
      setZoom(value);
    };
    if (!animate) return display(next);
    const from = shown.current;
    const start = performance.now();
    const tick = (now: number) => {
      const t = Math.min(1, Math.max(0, now - start) / STEP_MS);
      // Ease out (cubic).
      display(t < 1 ? between(from, next, 1 - (1 - t) ** 3) : next);
      animation.current = t < 1 ? requestAnimationFrame(tick) : null;
    };
    animation.current = requestAnimationFrame(tick);
  }, []);
  /** A discrete step (a button, a key, a double click), from where the zoom is heading. */
  const step = useCallback((update: (zoom: Zoom) => Zoom) => show(update(target.current), true), [show]);

  // A gesture sends events faster than frames: they are gathered and applied once per frame.
  const queued = useRef<((zoom: Zoom) => Zoom)[]>([]);
  const frame = useRef<number | null>(null);
  const follow = useCallback(
    (update: (zoom: Zoom) => Zoom) => {
      queued.current.push(update);
      frame.current ??= requestAnimationFrame(() => {
        frame.current = null;
        const updates = queued.current;
        queued.current = [];
        show(
          updates.reduce((z, apply) => apply(z), target.current),
          false,
        );
      });
    },
    [show],
  );
  useEffect(
    () => () => {
      if (frame.current != null) cancelAnimationFrame(frame.current);
      if (animation.current != null) cancelAnimationFrame(animation.current);
    },
    [],
  );

  /** Another image of the set: it opens fitted, and is laid out once loaded. */
  const move = useCallback(
    (delta: number) => {
      const next = galleryStep(index, delta, count);
      if (next === index) return;
      queued.current = [];
      show(FITTED, false);
      setNatural(null);
      setAt({ index: next, image: images[next] });
    },
    [index, count, images, show],
  );

  /** A point of the window, relative to the centre of the area (that of the fitted image). */
  const fromCentre = useCallback((clientX: number, clientY: number) => {
    const rect = area.current!.getBoundingClientRect();
    return { x: clientX - (rect.left + rect.width / 2), y: clientY - (rect.top + rect.height / 2) };
  }, []);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const key: KeyAction | undefined = KEYS[event.key];
      if (!key) return;
      event.preventDefault();
      event.stopPropagation();
      if (key === "close") onClose();
      else if (key === "previous" || key === "next") move(key === "next" ? 1 : -1);
      else if (key === "fit") step(() => FITTED);
      else step((z) => zoomAt(z, key === "in" ? STEP : 1 / STEP));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose, step, move]);

  // A wheel listener that is not passive, to keep the webview from zooming or scrolling itself.
  useEffect(() => {
    const element = area.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      // A trackpad pinch arrives as a wheel with ctrlKey.
      if (event.ctrlKey || event.metaKey) {
        const at = fromCentre(event.clientX, event.clientY);
        const factor = wheelFactor(event.deltaY);
        follow((z) => zoomAt(z, factor, at));
      } else {
        const dx = -event.deltaX;
        const dy = -event.deltaY;
        follow((z) => pan(z, dx, dy));
      }
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  }, [follow, fromCentre]);

  useEffect(() => {
    const element = area.current;
    if (!element) return;
    const observer = new ResizeObserver(() => setBox({ width: element.clientWidth, height: element.clientHeight }));
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  const fitted = natural && box ? fitSize(natural, box) : null;
  /** The fitted size over the image's own: 100% is `1 / fit` times the fitted size. */
  const fit = natural && fitted ? fitted.width / natural.width : null;
  const pixelRatio = window.devicePixelRatio || 1;
  const placed = fitted ? placement(zoom, fitted, pixelRatio) : null;

  const zoomed = zoom.scale > 1;
  const percent = fit ? Math.round(fit * zoom.scale * 100) : null;

  if (followed === null) return null;
  return createPortal(
    <div
      data-popover
      className="fixed inset-0 z-[70] overflow-hidden bg-black/85"
      role="dialog"
      aria-label={name}
      onClick={(event) => event.target === event.currentTarget && onClose()}
    >
      <div
        ref={area}
        className={cx("absolute inset-x-8 bottom-14 top-8", zoomed && (dragging ? "cursor-grabbing" : "cursor-grab"))}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          drag.current = { x: event.clientX, y: event.clientY, moved: false };
          if (zoomed) event.currentTarget.setPointerCapture(event.pointerId);
        }}
        onPointerMove={(event) => {
          const start = drag.current;
          if (!start || !zoomed) return;
          const dx = event.clientX - start.x;
          const dy = event.clientY - start.y;
          if (!start.moved && Math.hypot(dx, dy) < 3) return;
          if (!start.moved) setDragging(true);
          drag.current = { x: event.clientX, y: event.clientY, moved: true };
          follow((z) => pan(z, dx, dy));
        }}
        onPointerUp={(event) => {
          const start = drag.current;
          drag.current = null;
          setDragging(false);
          if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
          // A click beside the fitted image closes; one that ends a drag does not.
          if (start && !start.moved && !zoomed && event.target === event.currentTarget) onClose();
        }}
        onDoubleClick={(event) => {
          if (zoomed) return step(() => FITTED);
          // To the image's own size when it was shrunk to fit, else twice as big.
          const factor = fit && fit < 1 ? 1 / fit : 2;
          const at = fromCentre(event.clientX, event.clientY);
          step(() => zoomAt(FITTED, factor, at));
        }}
      >
        {src && (
          <img
            // Two images of the set may share a src: a new element loads, and lays out, each one.
            key={`${index} ${src}`}
            src={src}
            alt={name}
            draggable={false}
            onLoad={(event) => {
              const img = event.currentTarget;
              // An SVG without a size of its own keeps the one it was given.
              const own = img.naturalWidth > 0 && img.naturalHeight > 0;
              setNatural({ width: own ? img.naturalWidth : img.clientWidth || 1, height: own ? img.naturalHeight : img.clientHeight || 1 });
            }}
            className="absolute top-1/2 left-1/2 max-w-none select-none rounded-md shadow-2xl"
            style={
              fitted && placed
                ? {
                    width: fitted.width,
                    height: fitted.height,
                    transform: `translate(${placed.x}px, ${placed.y}px) scale(${zoom.scale})`,
                    transformOrigin: "0 0",
                  }
                : { visibility: "hidden" }
            }
          />
        )}
      </div>
      {count > 1 && (
        <>
          <SideButton side="left" disabled={index === 0} onClick={() => move(-1)} />
          <SideButton side="right" disabled={index === count - 1} onClick={() => move(1)} />
        </>
      )}
      <div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center text-[12px] text-muted">
        <div className="pointer-events-auto flex items-center gap-0.5 rounded-md border border-line-strong bg-raised/95 p-0.5 pl-2.5 shadow-lg">
          <span className="mr-1.5 max-w-[40vw] truncate">{name}</span>
          {count > 1 && (
            <span className="mr-1.5 tabular-nums text-faint">
              {index + 1} / {count}
            </span>
          )}
          <ZoomButton title="Zoom out (−)" disabled={!zoomed} onClick={() => step((z) => zoomAt(z, 1 / STEP))}>
            <Minus size={13} />
          </ZoomButton>
          <button
            type="button"
            title="Fit to the window (0)"
            onClick={() => step(() => FITTED)}
            className="min-w-[48px] rounded px-1.5 py-0.5 text-center tabular-nums hover:bg-hover hover:text-fg"
          >
            {percent != null ? `${percent}%` : "Fit"}
          </button>
          <ZoomButton title="Zoom in (+)" onClick={() => step((z) => zoomAt(z, STEP))}>
            <Plus size={13} />
          </ZoomButton>
        </div>
      </div>
    </div>,
    document.body,
  );
}

function SideButton({ side, disabled, onClick }: { side: "left" | "right"; disabled: boolean; onClick: () => void }) {
  const title = side === "left" ? "Previous image (←)" : "Next image (→)";
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className={cx(
        "absolute top-1/2 flex h-10 w-7 -translate-y-1/2 items-center justify-center rounded-md bg-raised/80 text-muted hover:bg-hover hover:text-fg disabled:opacity-30 disabled:hover:bg-raised/80",
        side === "left" ? "left-0.5" : "right-0.5",
      )}
    >
      {side === "left" ? <ChevronLeft size={16} /> : <ChevronRight size={16} />}
    </button>
  );
}

function ZoomButton({
  title,
  disabled,
  onClick,
  children,
}: {
  title: string;
  disabled?: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      aria-label={title}
      disabled={disabled}
      onClick={onClick}
      className="flex h-6 w-6 items-center justify-center rounded hover:bg-hover hover:text-fg disabled:opacity-40 disabled:hover:bg-transparent"
    >
      {children}
    </button>
  );
}
