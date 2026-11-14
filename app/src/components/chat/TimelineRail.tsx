import { type MutableRefObject, type RefObject, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { VirtuosoHandle } from "react-virtuoso";
import { revealChatRow } from "@/lib/chatRowScroll";
import type { ChatRow } from "@/lib/chat";
import {
  markTime,
  placeTicks,
  pixelTicks,
  railShown,
  rowLayout,
  sameTicks,
  timelineMarks,
  type TimelineMark,
  type TimelineTick,
} from "@/lib/timeline";
import { cx } from "../ui";

/**
 * A thin rail along the right edge of the chat, one tick per user message placed where it sits in
 * the whole conversation. Hovering a tick previews the message; clicking it scrolls there and
 * flashes it. The list is virtualised: rows never shown yet count at the size the list estimates.
 */
export function TimelineRail({
  rows,
  list,
  scroller,
  header,
  refresh,
  jumping,
  navigation,
  onNavigate,
  onArrive,
  onUserScroll,
}: {
  rows: readonly ChatRow[];
  list: RefObject<VirtuosoHandle | null>;
  /** The list's scroll area. */
  scroller: HTMLElement | null;
  /** The height above the first row, in pixels. */
  header: number;
  /** Set to the rail's measure: the list calls it when its height changes. */
  refresh: MutableRefObject<() => void>;
  /** True while a jump scrolls: the list must not follow new output meanwhile. */
  jumping: MutableRefObject<boolean>;
  /** Cancellation shared with Latest, find and other chat navigation. */
  navigation: MutableRefObject<(() => void) | null>;
  /** A new manual destination supersedes a pending global search result. */
  onNavigate: () => void;
  /** The message a jump reached, to flash it. */
  onArrive: (key: string) => void;
  /** The wheel over a tick is a user scroll, although the tick is outside the scroller. */
  onUserScroll: (deltaY: number) => void;
}) {
  const previousMarks = useRef<readonly TimelineMark[]>([]);
  const marks = useMemo(() => (previousMarks.current = timelineMarks(rows, previousMarks.current)), [rows]);
  const [rail, setRail] = useState<{ shown: boolean; ticks: TimelineTick[] }>({ shown: false, ticks: [] });
  const [hovered, setHovered] = useState<string | null>(null);
  const latest = useRef({ marks, count: rows.length, hovered });
  latest.current = { marks, count: rows.length, hovered };

  // At most one measure per frame, however many tokens stream in or scroll events come.
  const frame = useRef(0);
  const measure = useCallback(() => {
    if (frame.current || !scroller) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      list.current?.getState(({ ranges }) => {
        const { marks, count, hovered } = latest.current;
        const viewport = {
          scrollTop: scroller.scrollTop,
          clientHeight: scroller.clientHeight,
          scrollHeight: scroller.scrollHeight,
          header,
        };
        const shown = railShown(marks, viewport);
        const ticks = shown ? pixelTicks(placeTicks(marks, rowLayout(ranges, count), viewport), scroller.clientHeight - 24, hovered) : [];
        setRail((current) => (current.shown === shown && sameTicks(current.ticks, ticks) ? current : { shown, ticks }));
      });
    });
  }, [list, scroller, header]);

  useEffect(() => {
    refresh.current = measure;
    return () => {
      refresh.current = () => {};
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    };
  }, [measure, refresh]);

  useEffect(() => {
    if (!scroller) return;
    scroller.addEventListener("scroll", measure, { passive: true });
    const resize = new ResizeObserver(measure);
    resize.observe(scroller);
    return () => {
      scroller.removeEventListener("scroll", measure);
      resize.disconnect();
    };
  }, [scroller, measure]);

  useEffect(measure, [marks, measure]);

  // A jump under way: cancelled by the next one or by unmounting.
  const mine = useRef<(() => void) | null>(null);
  useEffect(() => () => mine.current?.(), []);

  const jump = (tick: TimelineTick) => {
    onNavigate();
    setHovered(null);
    mine.current = revealChatRow(list.current, scroller, tick.index, "start", navigation, jumping, () => onArrive(tick.key));
  };

  if (!rail.shown) return null;
  const preview = rail.ticks.find((tick) => tick.key === hovered) ?? null;
  return (
    <nav
      aria-label="Your messages in this chat"
      // Off the scrollbar (10px); only the ticks catch the pointer, the wheel scrolls the chat between them.
      className="pointer-events-none absolute bottom-3 right-[10px] top-3 z-10 w-3.5"
      data-testid="timeline-rail"
    >
      {rail.ticks.map((tick, index) => (
        <button
          key={tick.key}
          type="button"
          aria-label={`Jump to your message: ${tick.preview}`}
          onClick={() => jump(tick)}
          onMouseEnter={() => setHovered(tick.key)}
          onMouseLeave={() => setHovered((current) => (current === tick.key ? null : current))}
          onFocus={() => setHovered(tick.key)}
          onBlur={() => setHovered((current) => (current === tick.key ? null : current))}
          // Sitting over the list, a tick would stop the wheel: it scrolls the chat as around it.
          onWheel={(event) => {
            if (event.ctrlKey || Math.abs(event.deltaX) >= Math.abs(event.deltaY)) return;
            onUserScroll(event.deltaY);
            const unit = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? scroller?.clientHeight ?? 1 : 1;
            scroller?.scrollBy({ top: event.deltaY * unit });
          }}
          style={{
            top: `${tick.top * 100}%`,
            height: Math.min(
              8,
              index > 0 ? (tick.top - rail.ticks[index - 1].top) * Math.max(1, (scroller?.clientHeight ?? 24) - 24) : 8,
              index + 1 < rail.ticks.length ? (rail.ticks[index + 1].top - tick.top) * Math.max(1, (scroller?.clientHeight ?? 24) - 24) : 8,
            ),
          }}
          className="pointer-events-auto absolute right-0 flex w-3.5 -translate-y-1/2 items-center justify-end rounded-sm"
          data-testid="timeline-tick"
        >
          <span
            className={cx(
              "pointer-events-none block h-[2px] shrink-0 rounded-full transition-[width,background-color] duration-150",
              hovered === tick.key
                ? "w-3.5 bg-accent"
                : tick.inView
                  ? "w-2.5 bg-muted/40 group-hover/list:bg-muted/80"
                  : "w-2 bg-faint/25 group-hover/list:bg-faint/60",
            )}
          />
        </button>
      ))}
      {preview && <TickPreview tick={preview} />}
    </nav>
  );
}

function TickPreview({ tick }: { tick: TimelineTick }) {
  const time = markTime(tick.ts);
  // Near an end of the rail it opens inwards, to stay in the chat.
  const shift = tick.top < 0.12 ? "-12%" : tick.top > 0.88 ? "-88%" : "-50%";
  return (
    <div
      role="tooltip"
      style={{ top: `${tick.top * 100}%`, transform: `translateY(${shift})` }}
      className="pointer-events-none absolute right-full mr-2 w-64 rounded-md border border-line-strong bg-raised px-2.5 py-1.5 text-[12px] leading-snug shadow-lg"
    >
      <div className="line-clamp-2 break-words text-fg">{tick.preview}</div>
      {time && <div className="mt-0.5 text-[11px] text-faint">{time}</div>}
    </div>
  );
}
