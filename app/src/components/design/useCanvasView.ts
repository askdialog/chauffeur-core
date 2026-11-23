// The view of a canvas (story 47): fitted to the page when first shown with screens and when
// another page is picked, kept fitted (the area resized, screens added) until the user pans or
// zooms. Shared by the app's canvas and a share's reviewer page.

import { useCallback, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CanvasResult, DesignViewport } from "@chauffeur/protocol";
import { type CanvasLayout, fitView, pageSize, rowTop, type Size, type View, zoomCentred } from "@/lib/canvas";
import { commentView, type Thread, THREAD_VIEW_MIN_WIDTH } from "@/lib/comments";
import type { DesignFollow } from "./DesignFollow";

export function useCanvasView(
  layout: CanvasLayout,
  pageIndex: number,
  canvas: CanvasResult | null,
  setPage: (page: number) => void,
  follow?: DesignFollow,
) {
  const [zoom, setZoom] = useState<View>({ scale: 1, x: 40, y: 40 });
  const [area, setArea] = useState<Size | null>(null);
  const commentsPanel = useRef<HTMLDivElement>(null);
  const [target, setTarget] = useState<{ page: number; thread: Thread; coveredWidth: number } | null>(null);
  const fitted = useRef<{ page: number; auto: boolean } | null>(null);
  const remote = follow?.remote;
  const navigate = follow?.navigate;
  const localView = useMemo<DesignViewport | null>(() => {
    if (!area?.width || !area.height || !layout.sections.length) return null;
    const size = pageSize(layout, zoom.scale);
    return {
      page: pageIndex,
      scale: zoom.scale,
      presentation: null,
      fullscreen: null,
      center_x: (area.width / 2 - zoom.x) / Math.max(1, size.width),
      center_y: (area.height / 2 - zoom.y) / Math.max(1, size.height),
    };
  }, [area, layout, pageIndex, zoom]);

  useLayoutEffect(() => {
    if (remote && canvas && remote.page < Math.max(canvas.pages.length, 1)) {
      if (remote.page !== pageIndex) {
        setPage(remote.page);
        return;
      }
      if (!area?.width || !area.height) return;
      const { scale, center_x, center_y } = remote;
      const size = pageSize(layout, scale);
      const x = area.width / 2 - center_x * size.width;
      const y = area.height / 2 - center_y * size.height;
      fitted.current = { page: pageIndex, auto: false };
      setTarget(null);
      setZoom((view) => (
        view.scale === scale && view.x === x && view.y === y ? view : { scale, x, y }
      ));
      return;
    }
    if (!area || area.width === 0 || area.height === 0 || layout.sections.length === 0) return;
    if (target?.page === pageIndex) {
      fitted.current = { page: pageIndex, auto: false };
      setZoom((view) => commentView(layout, view, { ...area, width: area.width - target.coveredWidth }, target.thread));
      setTarget(null);
      return;
    }
    if (fitted.current?.page === pageIndex && !fitted.current.auto) return;
    fitted.current = { page: pageIndex, auto: true };
    setZoom(fitView(layout, area));
  }, [area, layout, pageIndex, target, remote, setPage, canvas]);
  /** The user moved the view: it stays where they put it. */
  const moveView = useCallback((update: React.SetStateAction<View>) => {
    navigate?.();
    if (fitted.current) fitted.current.auto = false;
    setZoom(update);
  }, [navigate]);
  const fit = useCallback(() => {
    navigate?.();
    if (!area) return;
    fitted.current = { page: pageIndex, auto: true };
    setZoom(fitView(layout, area));
  }, [area, layout, pageIndex, navigate]);
  const zoomTo = useCallback((scale: number) => area && moveView((z) => zoomCentred(layout, z, scale, area)), [area, layout, moveView]);
  const reveal = useCallback((file: string) => {
    navigate?.();
    if (!area) return;
    for (const [index, section] of layout.sections.entries()) {
      const screen = section.screens.find((screen) => screen.screen.file === file);
      if (!screen) continue;
      const left = zoom.x + zoom.scale * screen.x;
      const top = rowTop(layout, zoom, index);
      const width = zoom.scale * screen.width;
      const height = zoom.scale * screen.height;
      if (left < 0 || top < 0 || left + width > area.width || top + height > area.height) {
        moveView((view) => ({ ...view, x: (area.width - width) / 2 - zoom.scale * screen.x, y: view.y + (area.height - height) / 2 - top }));
      }
      return;
    }
  }, [layout, zoom, area, moveView, navigate]);
  /** Reveals the thread once its page is laid out. Returns true if the panel needs to close. */
  const revealComment = useCallback((thread: Thread) => {
    navigate?.();
    const file = thread.comment.screen;
    const currentPageHasFile = layout.sections.some((section) => section.screens.some((screen) => screen.screen.file === file));
    const page = currentPageHasFile ? pageIndex : canvas?.pages.findIndex((entry) =>
      entry.sections.some((section) => section.screens.some((screen) => screen.file === file))) ?? -1;
    if (page < 0) return false;
    const coveredWidth = commentsPanel.current?.offsetWidth ?? 0;
    const closePanel = coveredWidth > 0 && (!area || area.width - coveredWidth < THREAD_VIEW_MIN_WIDTH);
    setPage(page);
    setTarget({ page, thread, coveredWidth: closePanel ? 0 : coveredWidth });
    return closePanel;
  }, [area, canvas, layout, pageIndex, setPage, navigate]);
  return { localView, zoom, area, setArea, moveView, fit, zoomTo, reveal, revealComment, commentsPanel };
}
