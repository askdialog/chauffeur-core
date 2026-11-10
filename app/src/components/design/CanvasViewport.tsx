// The canvas's viewport (story 47): the screens on one scaled layer, their sharp pictures over
// them where the app takes some (see useScreenSnapshots), their titles, labels and outlines at
// screen size, pan and zoom by pointer, wheel, pinch or keys, a click that selects
// or, in Comment mode (story 51), pins a comment; the pins and the popover of the thread or
// bubble open. It holds no state of the design: the app's canvas and the reviewer page of a
// share both drive it.

import type { CanvasScreen } from "@chauffeur/protocol";
import { ImageDown, Maximize2 } from "lucide-react";
import { memo, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  type CanvasLayout,
  gridSpacing,
  headerTop,
  LABEL_HEIGHT,
  panBy,
  rowLayerY,
  rowTop,
  type Size,
  TITLE_HEIGHT,
  toggleSelection,
  type View,
  visibleScreens,
  wheelZoom,
  zoomAround,
} from "@/lib/canvas";
import { type Draft, type DrawnPin, popoverAnchor, type Thread } from "@/lib/comments";
import type { Snapshot } from "@/lib/screenSnapshots";
import { cx, IconButton } from "../ui";
import { PublishedLink } from "./CanvasOverlays";
import { PinButton, PinPopover } from "./ScreenComments";
import { ScreenFrame } from "./ScreenFrame";

type GestureLike = Event & { scale: number; clientX: number; clientY: number };

/** Whether an element from `target` up to `boundary` can still scroll along the wheel's delta. */
function scrollsWithin(target: Element, boundary: Element, dx: number, dy: number): boolean {
  for (let node: Element | null = target; node; node = node === boundary ? null : node.parentElement) {
    const style = getComputedStyle(node);
    // A textarea scrolls its text whatever overflow its style reports.
    const textarea = node instanceof HTMLTextAreaElement;
    const scrolls = (overflow: string) => textarea || overflow === "auto" || overflow === "scroll";
    if (dy !== 0 && scrolls(style.overflowY) && (dy < 0 ? node.scrollTop > 0 : Math.ceil(node.scrollTop) + node.clientHeight < node.scrollHeight)) return true;
    if (dx !== 0 && scrolls(style.overflowX) && (dx < 0 ? node.scrollLeft > 0 : Math.ceil(node.scrollLeft) + node.clientWidth < node.scrollWidth)) return true;
  }
  return false;
}

const CanvasFrame = memo(function CanvasFrame({ id, screen, scale, onFrame }: {
  id: string;
  screen: CanvasScreen;
  scale: number;
  onFrame: (id: string, file: string, frame: HTMLIFrameElement | null, loaded: boolean) => void;
}) {
  const track = useCallback((frame: HTMLIFrameElement | null, loaded: boolean) => onFrame(id, screen.file, frame, loaded), [id, screen.file, onFrame]);
  return <ScreenFrame screen={screen} scale={scale} interactive={false} capture onFrame={track} />;
});

export function CanvasViewport({
  layout,
  zoom,
  area,
  setZoom,
  onArea,
  selection,
  onSelect,
  flash,
  onFullScreen,
  onExport,
  onFit,
  onActualSize,
  published,
  commentMode,
  pins,
  draft,
  openThread,
  onFrame,
  onCommentAt,
  onOpenThread,
  onEscape,
  onToggleCommentMode,
  popover,
  snapshots,
}: {
  layout: CanvasLayout;
  zoom: View;
  area: Size | null;
  setZoom: (update: React.SetStateAction<View>) => void;
  onArea: (size: Size) => void;
  selection: string[];
  onSelect: (files: string[]) => void;
  flash: Record<string, number>;
  onFullScreen: (section: number) => void;
  /** Export PNG on a section's title; none on a share's reviewer page. */
  onExport?: (section: number) => void;
  onFit: () => void;
  onActualSize: () => void;
  /** The claude.ai URL of each published screen, by file. */
  published: Record<string, string>;
  commentMode: boolean;
  /** The pins to draw, by file. */
  pins: Record<string, DrawnPin[]>;
  draft: Draft | null;
  openThread: Thread | null;
  onFrame: (file: string, frame: HTMLIFrameElement | null, loaded: boolean) => void;
  /** A click in Comment mode: on a screen at a point (percent), or on its label for the whole screen. */
  onCommentAt: (file: string, x: number, y: number, whole: boolean, frame?: HTMLIFrameElement | null) => void;
  onOpenThread: (id: string) => void;
  onEscape: () => void;
  onToggleCommentMode: () => void;
  /** The bubble or thread shown at the draft's or open thread's pin. */
  popover: ReactNode;
  /** The picture laid over each screen occurrence, by its layout id; none on a share's reviewer page. */
  snapshots?: Record<string, Snapshot>;
}) {
  const element = useRef<HTMLDivElement>(null);
  const frames = useRef(new Map<string, { file: string; frame: HTMLIFrameElement }>());
  const trackFrame = useCallback((id: string, file: string, frame: HTMLIFrameElement | null, loaded: boolean) => {
    if (frame) frames.current.set(id, { file, frame });
    else frames.current.delete(id);
    // Removing one occurrence must not unregister another occurrence of the same file.
    const remaining = frame ?? [...frames.current.values()].find((entry) => entry.file === file)?.frame ?? null;
    onFrame(file, remaining, loaded);
  }, [onFrame]);
  const layoutRef = useRef(layout);
  layoutRef.current = layout;
  const [space, setSpace] = useState(false);
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ x: number; y: number; moved: boolean; file: string | null; screen: HTMLElement | null; additive: boolean } | null>(null);
  // Touch: two fingers pinch (their distance) around their midpoint; a drag ends when a second finger lands.
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinch = useRef<number | null>(null);

  // A gesture sends events faster than frames: they are gathered and applied once per frame.
  const queued = useRef<Array<(view: View) => View>>([]);
  const frame = useRef<number | null>(null);
  const follow = useCallback(
    (update: (view: View) => View) => {
      queued.current.push(update);
      frame.current ??= requestAnimationFrame(() => {
        frame.current = null;
        const updates = queued.current;
        queued.current = [];
        setZoom((view) => updates.reduce((v, apply) => apply(v), view));
      });
    },
    [setZoom],
  );
  useEffect(() => () => {
    if (frame.current != null) cancelAnimationFrame(frame.current);
  }, []);

  useEffect(() => {
    const el = element.current;
    if (!el) return;
    const observer = new ResizeObserver(() => onArea({ width: el.clientWidth, height: el.clientHeight }));
    observer.observe(el);
    return () => observer.disconnect();
  }, [onArea]);

  // Not passive, to keep the webview from scrolling or zooming itself. A trackpad pinch is a
  // wheel with ctrlKey (Chromium), or gesture events (WebKit), never both counted.
  useEffect(() => {
    const el = element.current;
    if (!el) return;
    const local = (x: number, y: number) => {
      const rect = el.getBoundingClientRect();
      return { x: x - rect.left, y: y - rect.top };
    };
    let gesture: number | null = null;
    const onWheel = (event: WheelEvent) => {
      const zooms = event.ctrlKey || event.metaKey;
      // Over the open thread or bubble, the wheel scrolls its text or replies, never the canvas.
      const target = event.target instanceof Element ? event.target : null;
      const popover = zooms ? null : target?.closest("[data-canvas-popover]");
      if (target && popover) {
        if (!scrollsWithin(target, popover, event.deltaX, event.deltaY)) event.preventDefault();
        return;
      }
      event.preventDefault();
      if (gesture != null) return;
      if (zooms) {
        const at = local(event.clientX, event.clientY);
        const factor = wheelZoom(event.deltaY, event.deltaMode);
        follow((v) => zoomAround(layoutRef.current, v, v.scale * factor, at));
      } else {
        const unit = event.deltaMode === 1 ? 16 : 1;
        const dx = -event.deltaX * unit;
        const dy = -event.deltaY * unit;
        follow((v) => panBy(v, dx, dy));
      }
    };
    const onGestureStart = (event: Event) => {
      event.preventDefault();
      gesture = (event as GestureLike).scale || 1;
    };
    const onGestureChange = (event: Event) => {
      event.preventDefault();
      const g = event as GestureLike;
      const factor = g.scale / (gesture || 1);
      gesture = g.scale;
      const at = local(g.clientX, g.clientY);
      follow((v) => zoomAround(layoutRef.current, v, v.scale * factor, at));
    };
    const onGestureEnd = (event: Event) => {
      event.preventDefault();
      gesture = null;
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    el.addEventListener("gesturestart", onGestureStart);
    el.addEventListener("gesturechange", onGestureChange);
    el.addEventListener("gestureend", onGestureEnd);
    return () => {
      el.removeEventListener("wheel", onWheel);
      el.removeEventListener("gesturestart", onGestureStart);
      el.removeEventListener("gesturechange", onGestureChange);
      el.removeEventListener("gestureend", onGestureEnd);
    };
  }, [follow]);

  const grid = gridSpacing(zoom.scale);
  const selected = useMemo(() => new Set(selection), [selection]);
  const visible = useMemo(() => visibleScreens(layout, zoom, area), [layout, zoom, area]);

  return (
    <div
      ref={element}
      tabIndex={0}
      role="application"
      aria-label="Canvas: drag to pan, pinch or ⌘-scroll to zoom, click a screen to select it"
      data-testid="canvas-viewport"
      className={cx(
        "absolute inset-0 touch-none overflow-hidden outline-none",
        dragging ? "cursor-grabbing" : space ? "cursor-grab" : commentMode ? "cursor-crosshair" : "cursor-default",
      )}
      style={{
        backgroundImage: "radial-gradient(circle, var(--color-line-strong) 1px, transparent 1.3px)",
        backgroundSize: `${grid}px ${grid}px`,
        backgroundPosition: `${zoom.x}px ${zoom.y}px`,
      }}
      onKeyDown={(event) => {
        if (event.key === " ") {
          event.preventDefault();
          if (!event.repeat) setSpace(true);
        } else if (event.key === "Escape") {
          // The canvas keeps Esc: it closes what is open or clears the selection, it does not interrupt the agent.
          event.preventDefault();
          event.stopPropagation();
          onEscape();
        } else if ((event.key === "c" || event.key === "C") && !event.metaKey && !event.ctrlKey && !event.altKey) {
          event.preventDefault();
          onToggleCommentMode();
        } else if (event.shiftKey && !event.metaKey && !event.ctrlKey && !event.altKey) {
          if (event.code === "Digit1") {
            event.preventDefault();
            onFit();
          } else if (event.code === "Digit0") {
            event.preventDefault();
            onActualSize();
          }
        }
      }}
      onKeyUp={(event) => {
        if (event.key === " ") setSpace(false);
      }}
      onBlur={() => setSpace(false)}
      onPointerDown={(event) => {
        if (event.button !== 0 && event.button !== 1) return;
        const target = event.target as HTMLElement;
        if (target.closest("button, [data-canvas-popover], [data-canvas-label]")) return;
        event.preventDefault();
        element.current?.focus({ preventScroll: true });
        pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
        event.currentTarget.setPointerCapture(event.pointerId);
        if (pointers.current.size === 2) {
          const [a, b] = [...pointers.current.values()];
          pinch.current = Math.hypot(a.x - b.x, a.y - b.y);
          drag.current = null;
          setDragging(false);
          return;
        }
        if (pointers.current.size > 2) return;
        const screen = target.closest<HTMLElement>("[data-canvas-screen]");
        drag.current = {
          x: event.clientX,
          y: event.clientY,
          moved: false,
          file: space ? null : (screen?.dataset.file ?? null),
          screen,
          additive: event.shiftKey,
        };
      }}
      onPointerMove={(event) => {
        if (pointers.current.has(event.pointerId)) pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
        if (pinch.current != null && pointers.current.size === 2) {
          const [a, b] = [...pointers.current.values()];
          const distance = Math.hypot(a.x - b.x, a.y - b.y);
          const factor = distance / (pinch.current || distance || 1);
          pinch.current = distance;
          const rect = element.current?.getBoundingClientRect();
          const at = rect ? { x: (a.x + b.x) / 2 - rect.left, y: (a.y + b.y) / 2 - rect.top } : { x: 0, y: 0 };
          follow((v) => zoomAround(layoutRef.current, v, v.scale * factor, at));
          return;
        }
        const start = drag.current;
        if (!start) return;
        const dx = event.clientX - start.x;
        const dy = event.clientY - start.y;
        if (!start.moved && Math.hypot(dx, dy) < 3) return;
        if (!start.moved) setDragging(true);
        drag.current = { ...start, x: event.clientX, y: event.clientY, moved: true };
        follow((v) => panBy(v, dx, dy));
      }}
      onPointerUp={(event) => {
        pointers.current.delete(event.pointerId);
        if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
        if (pinch.current != null) {
          // The pinch ends with its last finger; no click comes out of it.
          if (pointers.current.size === 0) pinch.current = null;
          return;
        }
        const start = drag.current;
        drag.current = null;
        setDragging(false);
        if (!start || start.moved || space) return;
        if (commentMode) {
          // A click on a screen comments at that point, in percent of the screen; beside the screens it closes what is open.
          const screen = start.screen;
          if (screen && start.file) {
            const rect = screen.getBoundingClientRect();
            const x = Math.min(100, Math.max(0, ((event.clientX - rect.left) / rect.width) * 100));
            const y = Math.min(100, Math.max(0, ((event.clientY - rect.top) / rect.height) * 100));
            onCommentAt(start.file, x, y, false, screen.querySelector("iframe"));
          } else {
            onEscape();
          }
          return;
        }
        // A click: on a screen it selects it (⇧ adds or takes out), beside the screens it clears.
        onSelect(start.file ? toggleSelection(selection, start.file, start.additive) : start.additive ? selection : []);
      }}
      onPointerCancel={(event) => {
        pointers.current.delete(event.pointerId);
        if (pointers.current.size === 0) pinch.current = null;
        drag.current = null;
        setDragging(false);
      }}
    >
      {/* The screens, at their own size, on one scaled layer. */}
      <div
        className="absolute left-0 top-0 h-0 w-0 origin-top-left"
        style={{ transform: `translate(${zoom.x}px, ${zoom.y}px) scale(${zoom.scale})` }}
      >
        {visible.map((screens, index) => {
          const top = rowLayerY(layout, zoom.scale, index);
          return screens.map((placed) => (
            <div
              key={placed.id}
              data-canvas-screen
              data-file={placed.screen.file}
              className="absolute overflow-hidden bg-white shadow-lg shadow-black/40"
              style={{ left: placed.x, top, width: placed.width, height: placed.height }}
            >
              {/* The scale only sizes the "file is missing" card: a served screen gets a constant one, so zooming does not re-render its memoized frame. */}
              <CanvasFrame id={placed.id} screen={placed.screen} scale={placed.screen.url ? 1 : zoom.scale} onFrame={trackFrame} />
              {/* Takes the pointer: on the canvas a screen is not interactive. */}
              <div className="absolute inset-0" />
            </div>
          ));
        })}
      </div>
      {/* The screens' pictures, laid out at their size on screen: a picture on the scaled layer
          would be drawn at 100% and shrunk by the GPU, as the frames are. */}
      {snapshots && (
        <div className="pointer-events-none absolute inset-0" data-testid="canvas-snapshots">
          {visible.map((screens, index) => {
            const row = rowTop(layout, zoom, index);
            return screens.map((placed) => {
              const picture = snapshots[placed.id];
              if (!picture) return null;
              return (
                <img
                  key={placed.id}
                  src={picture.url}
                  alt=""
                  draggable={false}
                  className="absolute max-w-none select-none"
                  style={{
                    left: zoom.x + zoom.scale * (placed.x + picture.rect.x),
                    top: row + zoom.scale * picture.rect.y,
                    width: zoom.scale * picture.rect.width,
                    height: zoom.scale * picture.rect.height,
                  }}
                />
              );
            });
          })}
        </div>
      )}
      {/* Titles, labels and outlines, at their size on screen whatever the zoom. */}
      <div className="pointer-events-none absolute inset-0">
        {layout.sections.map((section, index) => {
          const header = headerTop(layout, zoom, index);
          const row = rowTop(layout, zoom, index);
          const nearby = new Set(visible[index]);
          // A comment being written keeps its composer mounted when panned out of view.
          const overlays = section.screens.filter((placed) => nearby.has(placed)
            || placed.screen.file === draft?.file || placed.screen.file === openThread?.comment.screen);
          return (
            <div key={index}>
              <div
                className="absolute flex items-center gap-1 whitespace-nowrap"
                style={{ left: zoom.x, top: header, height: TITLE_HEIGHT }}
                data-testid="canvas-section"
              >
                <span className={cx("text-[13px] font-semibold", section.unsorted ? "text-muted" : "text-fg")}>{section.title}</span>
                <span className="text-[11.5px] text-faint">{section.screens.length}</span>
                <IconButton
                  title="Full screen: this section's screens at 100%, interactive"
                  className="pointer-events-auto"
                  onClick={() => onFullScreen(index)}
                  data-testid="canvas-full-screen"
                >
                  <Maximize2 size={12} />
                </IconButton>
                {onExport && (
                  <IconButton
                    title="Export PNG: this section's screens in a row, kept in the design folder's exports/ and put on the clipboard"
                    className="pointer-events-auto"
                    onClick={() => onExport(index)}
                    data-testid="canvas-section-export"
                  >
                    <ImageDown size={12} />
                  </IconButton>
                )}
              </div>
              {overlays.map((placed) => {
                const left = zoom.x + zoom.scale * placed.x;
                const width = zoom.scale * placed.width;
                const height = zoom.scale * placed.height;
                const file = placed.screen.file;
                const anchor = popover ? popoverAnchor(file, width, height, draft, openThread) : null;
                return (
                  <div key={placed.id}>
                    <div
                      className={cx(
                        "absolute flex items-center gap-0.5 text-[11.5px] leading-[22px]",
                        selected.has(file) ? "text-accent" : "text-muted",
                        commentMode && "pointer-events-auto cursor-crosshair hover:text-accent",
                      )}
                      style={{ left, top: row - LABEL_HEIGHT, width: Math.max(width, published[file] ? 40 : 24), height: LABEL_HEIGHT }}
                      data-canvas-label={commentMode ? "" : undefined}
                      onClick={commentMode ? () => onCommentAt(file, 0, 0, true) : undefined}
                      title={commentMode ? "Comment on the whole screen" : undefined}
                    >
                      <span className="min-w-0 truncate" title={file}>
                        {placed.screen.label || file}
                      </span>
                      {published[file] && <PublishedLink url={published[file]} />}
                    </div>
                    {selected.has(file) && (
                      <div
                        className="absolute rounded-[3px] ring-2 ring-accent"
                        style={{ left: left - 2, top: row - 2, width: width + 4, height: height + 4 }}
                        data-testid="canvas-selected"
                      />
                    )}
                    {flash[file] != null && (
                      <div
                        key={flash[file]}
                        className="canvas-flash absolute rounded-[3px] border-2 border-accent"
                        style={{ left: left - 3, top: row - 3, width: width + 6, height: height + 6 }}
                      />
                    )}
                    {(pins[file] ?? []).map((pin) => (
                      <PinButton
                        key={pin.thread.comment.id}
                        pin={pin}
                        open={openThread?.comment.id === pin.thread.comment.id}
                        left={pin.whole ? left + 10 : left + (width * pin.x) / 100}
                        top={pin.whole ? row + 10 : row + (height * pin.y) / 100}
                        onOpen={() => onOpenThread(pin.thread.comment.id)}
                      />
                    ))}
                    {anchor && (
                      <PinPopover left={left + anchor.x + 12} top={row + anchor.y - 8} onEscape={onEscape}>
                        {popover}
                      </PinPopover>
                    )}
                  </div>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}
