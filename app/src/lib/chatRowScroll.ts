import type { VirtuosoHandle } from "react-virtuoso";
import { CHAT_LIST_HEADER, chatRowPosition } from "./chatScroll";

/** A single movement, with no library-owned retries that could undo a later user scroll. */
export function scrollToChatRow(
  list: VirtuosoHandle | null,
  scroller: HTMLElement | null,
  index: number,
  align: "start" | "center" = "start",
  offset = 0,
) {
  if (!list || !scroller) return;
  list.getState(({ ranges }) => {
    const row = scroller.querySelector<HTMLElement>(`[data-index="${index}"]`);
    const measured = row?.getBoundingClientRect();
    const estimated = chatRowPosition(ranges, index);
    // A mounted row already includes the header in its measured position.
    const top = measured ? measured.top - scroller.getBoundingClientRect().top + scroller.scrollTop : CHAT_LIST_HEADER + estimated.top;
    const height = measured?.height ?? estimated.height;
    scroller.scrollTo({ top: top + (align === "center" ? (height - scroller.clientHeight) / 2 : 0) + offset });
  });
}

/** Refine an estimated row position until measured, yielding to the next navigation or input. */
export function revealChatRow(
  list: VirtuosoHandle | null,
  scroller: HTMLElement | null,
  index: number,
  align: "start" | "center",
  navigation: { current: (() => void) | null },
  jumping: { current: boolean },
  onArrive?: () => void,
): () => void {
  navigation.current?.();
  if (!list || !scroller) return () => {};
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
  const step = () => {
    const row = scroller.querySelector(`[data-index="${index}"]`);
    const box = row?.getBoundingClientRect();
    const viewport = scroller.getBoundingClientRect();
    const visible = box && box.bottom > viewport.top && box.top < viewport.bottom;
    const end = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 2;
    const difference = box ? box.top - viewport.top + (align === "center" ? (box.height - viewport.height) / 2 : 0) : Infinity;
    if (Math.abs(difference) <= 3 || (visible && (end || scroller.scrollTop <= 1))) settled++;
    else {
      settled = 0;
      scrollToChatRow(list, scroller, index, align);
    }
    if (settled >= 3 || ++tries >= 120) {
      cancel();
      if (settled >= 3) onArrive?.();
    } else frame = requestAnimationFrame(step);
  };
  frame = requestAnimationFrame(step);
  return cancel;
}
