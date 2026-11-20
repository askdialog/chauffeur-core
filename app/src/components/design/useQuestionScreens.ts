import type { CanvasResult } from "@chauffeur/protocol";
import { useEffect, useMemo, useState } from "react";

export function useQuestionScreens(
  canvas: CanvasResult | null, page: number, setPage: (page: number) => void,
  select: (files: string[]) => void, reveal: (file: string) => void,
) {
  const screens = useMemo(() => {
    const found = new Map<string, number>();
    canvas?.pages.forEach((entry, index) => {
      for (const section of entry.sections) {
        for (const screen of section.screens) {
          if (screen.url && !found.has(screen.file)) found.set(screen.file, index);
        }
      }
    });
    for (const screen of canvas?.unsorted ?? []) {
      if (screen.url && !found.has(screen.file)) found.set(screen.file, -1);
    }
    return found;
  }, [canvas]);
  const available = useMemo(() => new Set(screens.keys()), [screens]);
  const [target, setTarget] = useState<{ file: string; page: number } | null>(null);
  useEffect(() => {
    if (!target || target.page !== page) return;
    reveal(target.file);
    setTarget(null);
  }, [target, page, reveal]);
  const locate = (file: string) => {
    const index = screens.get(file);
    if (index == null) return;
    const destination = index < 0 ? page : index;
    if (destination !== page) setPage(destination);
    select([file]);
    setTarget({ file, page: destination });
  };
  return { available, locate };
}
