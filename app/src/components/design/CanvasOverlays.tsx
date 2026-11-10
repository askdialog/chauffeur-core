// The two ways to use a design chat's screens (story 47), over the whole app: a section full
// screen (its screens at 100% in a row that scrolls) and Present (one screen at a time, ← and →).
// The screens are interactive there. Comment mode (its button, C) pins comments on them as on the
// canvas (story 51): a cover then takes the clicks, and a label comments the whole screen. Esc
// closes the bubble or thread, then leaves Comment mode, then leaves; the close button leaves.

import type { CanvasScreen } from "@chauffeur/protocol";
import { ChevronLeft, ChevronRight, ExternalLink, Maximize2, MessageSquarePlus, Play, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { fitScale, type PresentStep, type ShownSection, type Size, stepPresent } from "@/lib/canvas";
import { openExternal } from "@/lib/native";
import { Button, cx, IconButton, Kbd } from "../ui";
import { type ScreenCommenting, ScreenComments } from "./ScreenComments";
import { ScreenFrame } from "./ScreenFrame";

/** Esc (and other keys) before anything else of the app: a dialog in front keeps them. */
function useOverlayKeys(onKey: (event: KeyboardEvent) => boolean) {
  const latest = useRef(onKey);
  latest.current = onKey;
  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      if (latest.current(event)) {
        event.preventDefault();
        event.stopPropagation();
      }
    };
    window.addEventListener("keydown", listener, true);
    return () => window.removeEventListener("keydown", listener, true);
  }, []);
}

/** Whether a key goes to a field being typed in (a comment's text), not to the overlay's shortcuts. */
function typing(event: KeyboardEvent): boolean {
  const target = event.target;
  return target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
}

/** The overlays' comment keys: Esc closes what is open of the comments (true), C toggles Comment mode. */
function commentKey(event: KeyboardEvent, commenting: ScreenCommenting | undefined): boolean {
  if (!commenting) return false;
  if (event.key === "Escape") {
    if (!commenting.draft && !commenting.openThread && !commenting.commentMode) return false;
    commenting.onEscape();
    return true;
  }
  if ((event.key === "c" || event.key === "C") && !event.metaKey && !event.ctrlKey && !event.altKey && !typing(event)) {
    commenting.onToggleCommentMode();
    return true;
  }
  return false;
}

function CommentModeButton({ commenting }: { commenting: ScreenCommenting }) {
  return (
    <Button
      size="sm"
      variant={commenting.commentMode ? "secondary" : "ghost"}
      onClick={commenting.onToggleCommentMode}
      title={commenting.commentMode ? "Leave Comment mode (C or Esc)" : "Comment mode (C): click a screen to comment on it, its label for the whole screen"}
      aria-pressed={commenting.commentMode}
      data-testid="overlay-comment-mode"
    >
      <MessageSquarePlus size={12} /> Comment
    </Button>
  );
}

/** A screen's label; in Comment mode a click on it comments the whole screen. */
function ScreenLabel({ screen, commenting, className }: { screen: CanvasScreen; commenting: ScreenCommenting | undefined; className?: string }) {
  const name = screen.label || screen.file;
  if (!commenting?.commentMode) {
    return (
      <span className={cx("min-w-0 truncate", className)} title={screen.file}>
        {name}
      </span>
    );
  }
  return (
    <button
      type="button"
      className={cx("min-w-0 cursor-crosshair truncate text-left hover:text-accent", className)}
      title="Comment on the whole screen"
      onClick={() => commenting.onCommentAt(screen.file, 0, 0, true)}
      data-testid="overlay-screen-label"
    >
      {name}
    </button>
  );
}

/** A screen at `scale`, interactive, with its comments over it when it can have some. */
function OverlayScreen({ screen, scale, commenting, className }: { screen: CanvasScreen; scale: number; commenting: ScreenCommenting | undefined; className: string }) {
  const box = useRef<HTMLDivElement>(null);
  const width = screen.width * scale;
  const height = screen.height * scale;
  return (
    <div className="relative shrink-0" style={{ width, height }}>
      <div ref={box} className={cx("overflow-hidden bg-white", className)} style={{ width, height }}>
        <div className="origin-top-left" style={{ width: screen.width, height: screen.height, transform: scale === 1 ? undefined : `scale(${scale})` }}>
          <ScreenFrame screen={screen} interactive capture={!!commenting} onFrame={commenting && ((frame, loaded) => commenting.onFrame(screen.file, frame, loaded))} />
        </div>
      </div>
      {commenting && <ScreenComments file={screen.file} width={width} height={height} frame={() => box.current?.querySelector("iframe") ?? null} commenting={commenting} />}
    </div>
  );
}

export function FullScreenSection({ section, onClose, commenting }: { section: ShownSection; onClose: () => void; commenting?: ScreenCommenting }) {
  useOverlayKeys((event) => {
    if (commentKey(event, commenting)) return true;
    if (event.key !== "Escape") return false;
    onClose();
    return true;
  });
  return createPortal(
    <div
      role="dialog"
      aria-label={`${section.title}, full screen`}
      data-popover
      className="fixed inset-0 z-[70] flex flex-col bg-bg"
      data-testid="canvas-full-screen-view"
    >
      <div className="flex h-10 shrink-0 items-center gap-2 border-b border-line px-4">
        <Maximize2 size={13} className="text-faint" />
        <span className="font-semibold text-fg">{section.title}</span>
        <span className="text-[12px] text-faint">
          {commenting?.commentMode
            ? "Click a screen to comment on it, its label for the whole screen"
            : `${section.screens.length === 1 ? "1 screen" : `${section.screens.length} screens`} at 100%, interactive`}
        </span>
        <span className="ml-auto flex items-center gap-2 text-[11.5px] text-faint">
          {commenting && <CommentModeButton commenting={commenting} />}
          <span>
            <Kbd>esc</Kbd> to leave
          </span>
        </span>
        <IconButton title="Leave full screen (Esc)" onClick={onClose} data-testid="canvas-close">
          <X size={14} />
        </IconButton>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <div className="flex w-max items-start gap-10 p-10">
          {section.screens.map((screen, index) => (
            <div key={`${index}:${screen.file}`} className="shrink-0">
              <div className="mb-2 flex text-[12px] text-muted" style={{ maxWidth: screen.width }}>
                <ScreenLabel screen={screen} commenting={commenting} />
              </div>
              <OverlayScreen screen={screen} scale={1} commenting={commenting} className="shadow-lg shadow-black/40" />
            </div>
          ))}
        </div>
      </div>
    </div>,
    document.body,
  );
}

/** "Open on claude.ai": a screen Claude published, in the browser. */
export function PublishedLink({ url, className }: { url: string; className?: string }) {
  return (
    <button
      type="button"
      title={`Open on claude.ai: ${url}`}
      aria-label="Open on claude.ai"
      onClick={() => void openExternal(url)}
      className={cx("pointer-events-auto inline-flex shrink-0 items-center rounded p-0.5 text-faint hover:bg-hover hover:text-accent", className)}
      data-testid="canvas-published"
    >
      <ExternalLink size={11} />
    </button>
  );
}

export function PresentScreens({
  steps,
  start,
  published,
  onClose,
  commenting,
}: {
  steps: PresentStep[];
  start: number;
  /** The claude.ai URL of each published screen, by file. */
  published: Record<string, string>;
  onClose: () => void;
  commenting?: ScreenCommenting;
}) {
  const [index, setIndex] = useState(start);
  const current = Math.min(index, steps.length - 1);
  const step = steps[current];
  const move = (direction: -1 | 1) => setIndex((i) => stepPresent(Math.min(i, steps.length - 1), steps.length, direction));
  useOverlayKeys((event) => {
    if (commentKey(event, commenting)) return true;
    if (event.key === "Escape") onClose();
    else if (typing(event)) return false;
    else if (event.key === "ArrowLeft") move(-1);
    else if (event.key === "ArrowRight") move(1);
    else return false;
    return true;
  });

  const stage = useRef<HTMLDivElement>(null);
  const [area, setArea] = useState<Size | null>(null);
  useEffect(() => {
    const el = stage.current;
    if (!el) return;
    const observer = new ResizeObserver(() => setArea({ width: el.clientWidth, height: el.clientHeight }));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);
  const scale = area && step ? fitScale(step.screen, area) : 1;

  return createPortal(
    <div role="dialog" aria-label="Present" data-popover className="fixed inset-0 z-[70] flex flex-col bg-black" data-testid="canvas-present-view">
      <div className="flex h-10 shrink-0 items-center gap-2 px-4 text-[12.5px]">
        <Play size={12} className="text-faint" />
        <span className="text-muted">{step?.section}</span>
        <span className="text-faint">/</span>
        {step && <ScreenLabel screen={step.screen} commenting={commenting} className="text-fg" />}
        {step && published[step.screen.file] && <PublishedLink url={published[step.screen.file]} />}
        <span className="ml-auto tabular-nums text-muted" data-testid="canvas-present-counter">
          {current + 1} / {steps.length}
        </span>
        {commenting && <CommentModeButton commenting={commenting} />}
        <IconButton title="Leave Present (Esc)" onClick={onClose} data-testid="canvas-close">
          <X size={14} />
        </IconButton>
      </div>
      <div className="relative flex min-h-0 flex-1 items-center gap-2 px-3 pb-3">
        <IconButton title="Previous screen (←)" onClick={() => move(-1)} disabled={current === 0} className="h-9 w-9 shrink-0">
          <ChevronLeft size={18} />
        </IconButton>
        <div ref={stage} className="flex h-full min-w-0 flex-1 items-center justify-center">
          {step && area && (
            <OverlayScreen key={`${current}:${step.screen.file}`} screen={step.screen} scale={scale} commenting={commenting} className="shadow-2xl shadow-black/60" />
          )}
        </div>
        <IconButton title="Next screen (→)" onClick={() => move(1)} disabled={current >= steps.length - 1} className="h-9 w-9 shrink-0">
          <ChevronRight size={18} />
        </IconButton>
      </div>
      <div className="flex h-7 shrink-0 items-center justify-center gap-3 text-[11.5px] text-faint">
        <span>
          <Kbd>←</Kbd> <Kbd>→</Kbd> between screens
        </span>
        {commenting && (
          <span>
            <Kbd>c</Kbd> to comment
          </span>
        )}
        <span>
          <Kbd>esc</Kbd> to leave
        </span>
        {scale < 1 && <span>shown at {Math.round(scale * 100)}%</span>}
      </div>
    </div>,
    document.body,
  );
}
