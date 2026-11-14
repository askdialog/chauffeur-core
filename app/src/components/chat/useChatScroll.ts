import { type MutableRefObject, type RefObject, useCallback, useEffect, useLayoutEffect, useRef } from "react";
import type { VirtuosoHandle } from "react-virtuoso";
import type { ListLocation } from "@/lib/chatList";
import { scrollToChatRow } from "@/lib/chatRowScroll";
import { afterChatScroll, atChatEnd, type ChatScrollIntent, type ChatScrollPosition } from "@/lib/chatScroll";

function position(scroller: HTMLElement): ChatScrollPosition {
  return { top: scroller.scrollTop, height: scroller.scrollHeight, viewport: scroller.clientHeight };
}

/** Keep an explicit request for the end alive through asynchronous row measurements. */
export function useChatScroll(
  list: RefObject<VirtuosoHandle | null>,
  scroller: HTMLElement | null,
  jumping: MutableRefObject<boolean>,
  navigation: MutableRefObject<(() => void) | null>,
  intent: MutableRefObject<ChatScrollIntent>,
  epoch: number,
  initial: ListLocation,
  onNavigate: () => void,
) {
  const frame = useRef(0);
  const measureFrame = useRef(0);
  const measuring = useRef(false);
  const before = useRef<ChatScrollPosition | null>(null);
  const refresh = useCallback(() => {
    if (frame.current || !scroller) return;
    const step = () => {
      frame.current = 0;
      if (jumping.current) {
        intent.current.following = false;
        // A jump ending on the last message resumes following even if no further scroll fires.
        frame.current = requestAnimationFrame(step);
        return;
      }
      if (intent.current.awaitingUserScroll) return;
      if (atChatEnd(position(scroller))) intent.current.following = true;
      if (intent.current.following && !atChatEnd(position(scroller))) {
        scroller.scrollTo({ top: scroller.scrollHeight });
      }
    };
    frame.current = requestAnimationFrame(step);
  }, [list, scroller, jumping, intent]);

  const measured = useCallback(() => {
    // Virtuoso compensates for row sizes after the measurement callback. Those scroll events
    // belong to the layout update; a real upward input still takes priority during these frames.
    measuring.current = true;
    cancelAnimationFrame(measureFrame.current);
    measureFrame.current = requestAnimationFrame(() => {
      measureFrame.current = requestAnimationFrame(() => {
        measureFrame.current = requestAnimationFrame(() => { measuring.current = false; });
      });
    });
    refresh();
  }, [refresh]);

  useLayoutEffect(() => {
    navigation.current?.();
    intent.current = { following: initial.index === "LAST", awaitingUserScroll: false };
    if (initial.index === "LAST" || !scroller) {
      refresh();
      return;
    }
    const index = initial.index;
    const offset = initial.offset ?? 0;
    let frame = 0;
    let settled = 0;
    let tries = 0;
    const cancel = () => {
      cancelAnimationFrame(frame);
      if (navigation.current === cancel) {
        navigation.current = null;
        jumping.current = false;
      }
    };
    navigation.current = cancel;
    jumping.current = true;
    const restore = () => {
      const row = scroller.querySelector(`[data-index="${index}"]`);
      const difference = row ? row.getBoundingClientRect().top - scroller.getBoundingClientRect().top + offset : Infinity;
      if (Math.abs(difference) <= 2) settled++;
      else {
        settled = 0;
        scrollToChatRow(list.current, scroller, index, "start", offset);
      }
      if (settled >= 3 || ++tries >= 120) { cancel(); refresh(); }
      else frame = requestAnimationFrame(restore);
    };
    frame = requestAnimationFrame(restore);
    return cancel;
    // Only a new list mount changes the initial destination.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [epoch, scroller]);

  const cancelNavigation = useCallback(() => {
    onNavigate();
    navigation.current?.();
  }, [onNavigate, navigation]);

  const pause = useCallback(() => {
    intent.current = { following: false, awaitingUserScroll: false };
    cancelNavigation();
  }, [cancelNavigation, intent]);

  const wheelFrame = useRef(0);
  const userScroll = useCallback((deltaY = -1) => {
    cancelNavigation();
    if (deltaY >= 0) { refresh(); return; }
    if (scroller) before.current = position(scroller);
    intent.current.awaitingUserScroll = true;
    cancelAnimationFrame(wheelFrame.current);
    wheelFrame.current = requestAnimationFrame(() => {
      wheelFrame.current = requestAnimationFrame(() => {
        intent.current.awaitingUserScroll = false;
        refresh();
      });
    });
  }, [cancelNavigation, intent, scroller, refresh]);
  useEffect(() => () => navigation.current?.(), [navigation]);

  useEffect(() => {
    if (!scroller) return;
    before.current = position(scroller);
    const wheel = (event: WheelEvent) => {
      if (event.ctrlKey || Math.abs(event.deltaX) >= Math.abs(event.deltaY)) return;
      cancelNavigation();
      if (event.deltaY >= 0 || scroller.scrollTop <= 0) return;
      // A code block, table or card with its own scroll area keeps its wheel movement.
      for (let el = event.target instanceof Element ? event.target : null; el && el !== scroller; el = el.parentElement) {
        const style = getComputedStyle(el);
        if (el.scrollHeight > el.clientHeight && /auto|scroll/.test(style.overflowY) &&
            (el.scrollTop > 0 || /contain|none/.test(style.overscrollBehaviorY))) return;
      }
      userScroll();
    };
    const pointer = (event: PointerEvent) => {
      cancelNavigation();
      if (event.clientX >= scroller.getBoundingClientRect().right - 12) userScroll();
    };
    const touch = (event: TouchEvent) => {
      if (event.touches.length === 1) userScroll();
      else cancelNavigation();
    };
    const key = (event: KeyboardEvent) => {
      if (event.target instanceof Element && event.target.closest("input, textarea, select, button, a, [contenteditable=true]")) return;
      if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(event.key)) {
        cancelNavigation();
        if (["ArrowUp", "PageUp", "Home"].includes(event.key) || (event.key === " " && event.shiftKey)) userScroll();
      }
    };
    const scroll = () => {
      const after = position(scroller);
      intent.current = afterChatScroll(intent.current, before.current ?? after, after, jumping.current, measuring.current);
      before.current = after;
      refresh();
    };
    scroller.addEventListener("wheel", wheel, { passive: true });
    scroller.addEventListener("pointerdown", pointer);
    scroller.addEventListener("touchstart", touch, { passive: true });
    scroller.addEventListener("scroll", scroll, { passive: true });
    scroller.addEventListener("keydown", key);
    const resize = new ResizeObserver(measured);
    resize.observe(scroller);
    return () => {
      scroller.removeEventListener("wheel", wheel);
      scroller.removeEventListener("pointerdown", pointer);
      scroller.removeEventListener("touchstart", touch);
      scroller.removeEventListener("scroll", scroll);
      scroller.removeEventListener("keydown", key);
      cancelAnimationFrame(wheelFrame.current);
      cancelAnimationFrame(measureFrame.current);
      resize.disconnect();
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    };
  }, [scroller, cancelNavigation, jumping, refresh, measured, userScroll, intent]);

  const toBottom = useCallback(() => {
    cancelNavigation();
    jumping.current = false;
    intent.current = { following: true, awaitingUserScroll: false };
    measured();
    scroller?.scrollTo({ top: scroller.scrollHeight });
  }, [cancelNavigation, jumping, measured, intent, scroller]);
  const toTop = useCallback(() => {
    pause();
    jumping.current = false;
    list.current?.scrollTo({ top: 0 });
  }, [list, jumping, pause]);
  return { refresh: measured, toBottom, toTop, pause, userScroll };
}
