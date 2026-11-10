// The view of a canvas (story 47): fitted to the page when first shown with screens and when
// another page is picked, kept fitted (the area resized, screens added) until the user pans or
// zooms. Shared by the app's canvas and a share's reviewer page.

import { useCallback, useLayoutEffect, useRef, useState } from "react";
import { type CanvasLayout, fitView, type Size, type View, zoomCentred } from "@/lib/canvas";

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
  return { zoom, area, setArea, moveView, fit, zoomTo };
}
