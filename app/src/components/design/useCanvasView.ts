// The view of a canvas (story 47): fitted to the page when first shown with screens and when
// another page is picked, kept fitted (the area resized, screens added) until the user pans or
// zooms. Shared by the app's canvas and a share's reviewer page.

import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { type CanvasLayout, fitView, rowTop, type Size, type View, zoomCentred } from "@/lib/canvas";

export function useCanvasView(layout: CanvasLayout, pageIndex: number) {
  const [zoom, setZoom] = useState<View>({ scale: 1, x: 40, y: 40 });
  const [area, setArea] = useState<Size | null>(null);
  const fitted = useRef<{ page: number; auto: boolean } | null>(null);
  useLayoutEffect(() => {
    if (!area || area.width === 0 || area.height === 0 || layout.sections.length === 0) return;
    if (fitted.current?.page === pageIndex && !fitted.current.auto) return;
    fitted.current = { page: pageIndex, auto: true };
    setZoom(fitView(layout, area));
  }, [area, layout, pageIndex]);
  /** The user moved the view: it stays where they put it. */
  const moveView = useCallback((update: React.SetStateAction<View>) => {
    if (fitted.current) fitted.current.auto = false;
    setZoom(update);
  }, []);
  const fit = useCallback(() => {
    if (!area) return;
    fitted.current = { page: pageIndex, auto: true };
    setZoom(fitView(layout, area));
  }, [area, layout, pageIndex]);
  const zoomTo = useCallback((scale: number) => area && moveView((z) => zoomCentred(layout, z, scale, area)), [area, layout, moveView]);
  const reveal = useCallback((file: string) => {
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
  }, [layout, zoom, area, moveView]);
  return { zoom, area, setArea, moveView, fit, zoomTo, reveal };
}
