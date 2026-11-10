// The pins of a canvas's comments (story 51), shared by the app's canvas and a share's reviewer
// page: which framed screens are mounted (the canvas's, and the one full screen or Present shows),
// which of the pins' selectors they still find (asked of a screen when it loads and when the
// threads change, see mergeFound), the pins to draw and the stale threads.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { PinElement } from "@chauffeur/protocol";
import { type DrawnPin, type FoundIn, mergeFound, pinsOf, selectorsToCheck, type Thread } from "@/lib/comments";
import { elementAt, selectorsFound } from "./captureBridge";

export function usePins(threads: readonly Thread[]) {
  const [found, setFound] = useState<Record<string, FoundIn>>({});
  const frames = useRef(new Map<string, HTMLIFrameElement>());
  const overlayFrames = useRef(new Map<string, HTMLIFrameElement>());
  const threadsRef = useRef(threads);
  threadsRef.current = threads;
  const checkScreen = useCallback(async (file: string, current: readonly Thread[]) => {
    const selectors = selectorsToCheck(current, file);
    if (selectors.length === 0) return;
    for (const frame of [frames.current.get(file), overlayFrames.current.get(file)]) {
      if (!frame) continue;
      const src = frame.getAttribute("src") ?? "";
      const answer = await selectorsFound(frame, selectors);
      if (answer) setFound((f) => ({ ...f, [file]: mergeFound(f[file], src, answer) }));
    }
  }, []);
  const track = useCallback(
    (map: Map<string, HTMLIFrameElement>, file: string, frame: HTMLIFrameElement | null, loaded: boolean) => {
      if (frame) map.set(file, frame);
      else map.delete(file);
      if (loaded) void checkScreen(file, threadsRef.current);
    },
    [checkScreen],
  );
  const onFrame = useCallback((file: string, frame: HTMLIFrameElement | null, loaded: boolean) => track(frames.current, file, frame, loaded), [track]);
  /** A frame of full screen or Present, mounted (null when gone) or loaded. */
  const onOverlayFrame = useCallback((file: string, frame: HTMLIFrameElement | null, loaded: boolean) => track(overlayFrames.current, file, frame, loaded), [track]);
  useEffect(() => {
    for (const file of new Set(threads.map((t) => t.comment.screen))) void checkScreen(file, threads);
  }, [threads, checkScreen]);
  const pins = useMemo(() => {
    const byFile: Record<string, DrawnPin[]> = {};
    for (const file of new Set(threads.map((t) => t.comment.screen))) byFile[file] = pinsOf(threads, file, found[file]?.selectors ?? null);
    return byFile;
  }, [threads, found]);
  const stale = useMemo(() => new Set(Object.values(pins).flat().filter((p) => p.stale).map((p) => p.thread.comment.id)), [pins]);
  /**
   * The element under a point of a screen, from `frame` (the one the user clicked, full screen or
   * in Present) else the canvas's; null when it does not answer.
   */
  const elementUnder = useCallback((file: string, x: number, y: number, frame?: HTMLIFrameElement | null): Promise<PinElement | null> => {
    const target = frame ?? frames.current.get(file);
    return target ? elementAt(target, x, y) : Promise.resolve(null);
  }, []);
  return { onFrame, onOverlayFrame, pins, stale, elementUnder };
}
