// The canvas's sharp pictures of its screens (story 47), on macOS (see lib/screenSnapshots.ts):
// taken natively once the view has stayed still a moment, the screen nearest the middle first,
// one at a time. Each screen keeps a few, and the one laid over it is chosen at every frame, so
// that the view is sharp as soon as it stops whenever a picture already serves it.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { type CanvasLayout, rowTop, screenSrc, type Size, type View } from "@/lib/canvas";
import { releaseScreenSnapshots, screenSnapshot } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { currentPicture, keepSnapshots, picturesToTake, pruneSnapshots, type Snapshot, shownSnapshot, snapshotKey, visiblePart } from "@/lib/screenSnapshots";

/** How long the view stays still before pictures are taken. */
const SETTLE_MS = 150;

/** The display's device pixel ratio, again when the window moves to another display. */
function useDevicePixelRatio(): number {
  const [ratio, setRatio] = useState(() => window.devicePixelRatio || 1);
  useEffect(() => {
    const query = window.matchMedia(`(resolution: ${ratio}dppx)`);
    const changed = () => setRatio(window.devicePixelRatio || 1);
    query.addEventListener("change", changed);
    return () => query.removeEventListener("change", changed);
  }, [ratio]);
  return ratio;
}

/** The picture to lay over each screen shown, by layout id; none when `enabled` is false. */
export function useScreenSnapshots(layout: CanvasLayout, view: View, area: Size | null, enabled: boolean): Record<string, Snapshot> {
  const ratio = useDevicePixelRatio();
  const pictures = useRef(new Map<string, Snapshot[]>());
  const failed = useRef(new Set<string>());
  const [taken, setTaken] = useState(0);
  const latest = useRef({ layout, view, area, ratio, enabled, moved: 0 });
  latest.current = { ...latest.current, layout, view, area, ratio, enabled };
  const running = useRef(false);
  const mounted = useRef(true);

  const take = useCallback(async () => {
    if (running.current) return;
    running.current = true;
    try {
      for (;;) {
        const { layout, view, area, ratio, moved, enabled } = latest.current;
        if (!mounted.current || !enabled || !area || performance.now() - moved < SETTLE_MS) return;
        const [request] = picturesToTake(layout, view, area, ratio, (id) => pictures.current.get(id) ?? [], failed.current);
        if (!request) return;
        try {
          const blob = await screenSnapshot(request);
          if (!mounted.current) return;
          if (!latest.current.enabled || !currentPicture(request, latest.current.layout)) continue;
          const added: Snapshot = { src: request.src, rect: request.rect, density: request.density, whole: request.whole, url: URL.createObjectURL(blob) };
          const { kept, dropped } = keepSnapshots(pictures.current.get(request.id) ?? [], added);
          pictures.current.delete(request.id);
          pictures.current.set(request.id, kept);
          const current = latest.current;
          dropped.push(...pruneSnapshots(pictures.current, current.layout, current.view, current.area));
          for (const picture of dropped) URL.revokeObjectURL(picture.url);
          setTaken((n) => n + 1);
        } catch (error) {
          // The live frame stays under it; this picture is not asked for again.
          failed.current.add(snapshotKey(request));
          console.warn(`could not picture ${request.file}`, errorMessage(error));
        }
      }
    } finally {
      running.current = false;
    }
  }, []);

  useEffect(() => {
    const dropped = pruneSnapshots(pictures.current, layout, view, area);
    for (const picture of dropped) URL.revokeObjectURL(picture.url);
    if (dropped.length) setTaken((n) => n + 1);
  }, [layout, view, area]);

  useEffect(() => {
    failed.current.clear();
  }, [layout]);

  useEffect(() => {
    if (!enabled) return;
    latest.current.moved = performance.now();
    const timer = setTimeout(() => void take(), SETTLE_MS);
    return () => clearTimeout(timer);
  }, [enabled, layout, view, area, ratio, take]);

  useEffect(() => {
    mounted.current = true;
    const kept = pictures.current;
    return () => {
      mounted.current = false;
      for (const list of kept.values()) for (const picture of list) URL.revokeObjectURL(picture.url);
      kept.clear();
      // The views are opened again for the next canvas.
      releaseScreenSnapshots().catch(() => {});
    };
  }, []);

  return useMemo(() => {
    const shown: Record<string, Snapshot> = {};
    if (!enabled || !area || taken === 0) return shown;
    layout.sections.forEach((section, index) => {
      const top = rowTop(layout, view, index);
      for (const placed of section.screens) {
        const src = screenSrc(placed.screen);
        const list = pictures.current.get(placed.id);
        if (!src || !list) continue;
        const visible = visiblePart(placed, view.x + view.scale * placed.x, top, view.scale, area);
        const picture = visible && shownSnapshot(list, src, visible, view.scale, ratio);
        if (picture) shown[placed.id] = picture;
      }
    });
    return shown;
  }, [enabled, area, layout, view, ratio, taken]);
}
