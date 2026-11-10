// Fitting a row of chips on one line, the rest collapsed into a `+N` chip.

/**
 * How many of the chips (their widths, in order) fit in `available`, leaving room for the `+N`
 * chip (`overflowWidth`) whenever some do not fit.
 */
export function fitCount(widths: number[], available: number, gap: number, overflowWidth: number): number {
  const used = (count: number) =>
    widths.slice(0, count).reduce((sum, width) => sum + width, 0) + gap * Math.max(0, count - 1);
  if (used(widths.length) <= available) return widths.length;
  for (let count = widths.length - 1; count > 0; count--) {
    if (used(count) + gap + overflowWidth <= available) return count;
  }
  return 0;
}
