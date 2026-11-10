// The sidebar's width, dragged from its right edge and kept across launches.

export const SIDEBAR_WIDTH_KEY = "chauffeur.sidebarWidth";
export const SIDEBAR_DEFAULT_WIDTH = 248;
/** Below it the rows' badges and the usage bars no longer fit. */
export const SIDEBAR_MIN_WIDTH = 200;
export const SIDEBAR_MAX_WIDTH = 480;

/** A width within the bounds, in whole pixels. */
export function clampSidebarWidth(width: number): number {
  return Math.round(Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, width)));
}

/** A stored width, or the default when it is missing or not a number. */
export function storedSidebarWidth(value: string | null): number {
  const width = value == null || value === "" ? NaN : Number(value);
  return Number.isFinite(width) ? clampSidebarWidth(width) : SIDEBAR_DEFAULT_WIDTH;
}
