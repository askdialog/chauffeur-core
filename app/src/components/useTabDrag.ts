// Reordering a strip's tabs by dragging one along it, as a browser's: the tab follows the pointer,
// the others make room, Esc cancels, the strip scrolls at its edges. Each tab carries
// `data-tab="<id>"`; the caller keeps the order and saves it in `onMove`.

import { type CSSProperties, type PointerEvent as ReactPointerEvent, type RefObject, useEffect, useRef, useState } from "react";
import { beforeOf, DRAG_THRESHOLD, dropIndex, edgeScroll, tabShift, type TabBox } from "@/lib/tabDrag";

type Drag = { id: string; from: number; to: number; dx: number; shift: number };

export function useTabDrag({
  strip,
  ids,
  onMove,
}: {
  strip: RefObject<HTMLElement | null>;
  /** The tabs' ids, in the order shown. */
  ids: string[];
  /** A tab dropped elsewhere: it goes before `before`, last without it. */
  onMove: (id: string, before: string | undefined) => void;
}) {
  const [drag, setDrag] = useState<Drag | null>(null);
  const suppressClick = useRef(false);
  const stop = useRef<(() => void) | null>(null);
  useEffect(() => () => stop.current?.(), []);

  const onPointerDown = (id: string, event: ReactPointerEvent) => {
    const from = ids.indexOf(id);
    const element = strip.current;
    if (event.button !== 0 || from < 0 || !element || (event.target as Element).closest("input")) return;
    const order = [...ids];
    const tab = event.currentTarget as HTMLElement;
    const pointerId = event.pointerId;
    const start = { x: event.clientX, y: event.clientY };
    let pointerX = event.clientX;
    let started: { boxes: TabBox[]; startX: number; shift: number } | null = null;
    let frame = 0;
    let last: Drag | null = null;

    const contentX = (x: number) => x - element.getBoundingClientRect().left + element.scrollLeft;
    const measure = () => {
      const origin = element.getBoundingClientRect().left - element.scrollLeft;
      const boxes = order.map((tabId) => {
        const rect = element.querySelector(`[data-tab="${CSS.escape(tabId)}"]`)?.getBoundingClientRect();
        return { left: (rect?.left ?? origin) - origin, width: rect?.width ?? 0 };
      });
      // The strip's gap between two tabs: the dragged one leaves its width and a gap.
      const [a, b] = boxes;
      const gap = a && b ? Math.max(0, b.left - a.left - a.width) : 0;
      return { boxes, startX: contentX(start.x), shift: (boxes[from]?.width ?? 0) + gap };
    };
    const update = () => {
      if (!started) return;
      const { boxes, startX, shift } = started;
      const box = boxes[from] as TabBox;
      const end = Math.max(...boxes.map((b) => b.left + b.width));
      const dx = Math.min(Math.max(contentX(pointerX) - startX, -box.left), end - box.left - box.width);
      last = { id, from, to: dropIndex(boxes, from, box.left + dx), dx, shift };
      setDrag(last);
    };
    // While dragged near an edge of a strip that overflows, the strip scrolls under the tab.
    const scroll = () => {
      frame = 0;
      if (!started) return;
      const rect = element.getBoundingClientRect();
      const by = element.scrollWidth > element.clientWidth ? edgeScroll(pointerX, rect.left, rect.right) : 0;
      if (by === 0) return;
      const before = element.scrollLeft;
      element.scrollLeft += by;
      if (element.scrollLeft !== before) update();
      frame = requestAnimationFrame(scroll);
    };

    const move = (e: PointerEvent) => {
      if (e.pointerId !== pointerId) return;
      pointerX = e.clientX;
      if (!started) {
        if (Math.hypot(e.clientX - start.x, e.clientY - start.y) < DRAG_THRESHOLD) return;
        started = measure();
        try {
          tab.setPointerCapture(pointerId);
        } catch {
          // released already: the window's listeners still follow it
        }
      }
      update();
      if (!frame) frame = requestAnimationFrame(scroll);
    };
    /** `held`: the pointer is still down (Esc), its release yet to come. */
    const end = (drop: boolean, held = false) => {
      stop.current?.();
      if (!started) return;
      setDrag(null);
      // The click that follows the release is not a click on the tab.
      suppressClick.current = true;
      const released = () => setTimeout(() => (suppressClick.current = false), 0);
      if (held) window.addEventListener("pointerup", released, { once: true, capture: true });
      else released();
      if (drop && last && last.to !== from) onMove(id, beforeOf(order, from, last.to));
    };
    const up = (e: PointerEvent) => e.pointerId === pointerId && end(true);
    const cancel = (e: PointerEvent) => e.pointerId === pointerId && end(false);
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !started) return;
      e.preventDefault();
      e.stopPropagation();
      end(false, true);
    };
    stop.current?.();
    stop.current = () => {
      stop.current = null;
      if (frame) cancelAnimationFrame(frame);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", cancel);
      window.removeEventListener("keydown", key, true);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", cancel);
    window.addEventListener("keydown", key, true);
  };

  /** How a tab is drawn during a drag: the dragged one under the pointer, the others aside. */
  const tabStyle = (id: string): CSSProperties | undefined => {
    if (!drag) return undefined;
    if (id === drag.id) {
      return { transform: `translateX(${drag.dx}px)`, position: "relative", zIndex: 1, cursor: "grabbing" };
    }
    const shift = tabShift(ids.indexOf(id), drag.from, drag.to, drag.shift);
    return { transform: shift ? `translateX(${shift}px)` : undefined, transition: "transform 150ms ease" };
  };

  return {
    onPointerDown,
    tabStyle,
    dragging: drag?.id ?? null,
    /** Whether a click is the end of a drag, not a click on the tab. */
    clickIsDrop: () => suppressClick.current,
  };
}
