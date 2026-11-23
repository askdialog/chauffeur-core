import type { DesignViewport } from "@chauffeur/protocol";
import { useLayoutEffect, useMemo, useState } from "react";
import type { PresentStep } from "@/lib/canvas";

type CanvasFollowView = {
  localView: DesignViewport | null;
  remote: DesignViewport | null | undefined;
  page: number;
  steps: PresentStep[];
  selection: string[];
  select: (files: string[]) => void;
};

/** Apply a presenter's overlay and selection locally, so taking control leaves both in place. */
export function useCanvasFollowView({ localView, remote, page, steps, selection, select }: CanvasFollowView) {
  const [presenting, setPresenting] = useState<number | null>(null);
  const [fullScreen, setFullScreen] = useState<number | null>(null);
  const onPage = remote?.page === page ? remote : null;

  useLayoutEffect(() => {
    if (!onPage) return;
    setPresenting(onPage.presentation);
    setFullScreen(onPage.fullscreen);
    const file = onPage.selected == null ? undefined : steps[onPage.selected]?.screen.file;
    if (file === undefined) {
      if (selection.length) select([]);
    } else if (selection.length !== 1 || selection[0] !== file) {
      select([file]);
    }
  }, [onPage, steps, selection, select]);

  const primary = selection[0];
  const selected = useMemo(() => primary ? steps.findIndex(step => step.screen.file === primary) : -1, [primary, steps]);
  const advertised = useMemo(() => {
    if (remote || !localView) return null;
    return { ...localView, presentation: presenting, fullscreen: fullScreen, selected: selected >= 0 ? selected : null };
  }, [remote, localView, presenting, fullScreen, selected]);

  return {
    advertised,
    shownPresentation: remote ? onPage?.presentation ?? null : presenting,
    shownFullscreen: remote ? onPage?.fullscreen ?? null : fullScreen,
    setPresenting,
    setFullScreen,
  };
}
