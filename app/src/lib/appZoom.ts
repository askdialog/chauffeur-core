// The zoom of the whole app (⌘+ / ⌘− / ⌘0), in the steps browsers use, kept across launches.

export const ZOOM_LEVELS = [0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2];
export const ZOOM_KEY = "chauffeur.zoom";

/** The next level up (1) or down (-1) from `current`, or 100% (0). */
export function stepZoom(current: number, direction: 1 | -1 | 0): number {
  if (direction === 0) return 1;
  if (direction === 1) return ZOOM_LEVELS.find((level) => level > current + 0.001) ?? ZOOM_LEVELS[ZOOM_LEVELS.length - 1];
  return [...ZOOM_LEVELS].reverse().find((level) => level < current - 0.001) ?? ZOOM_LEVELS[0];
}

/** A stored level, or 100% when it is missing or not one of the levels. */
export function storedZoom(value: string | null): number {
  const level = Number(value);
  return ZOOM_LEVELS.some((l) => Math.abs(l - level) < 0.001) ? level : 1;
}
