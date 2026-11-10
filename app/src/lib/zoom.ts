// The zoom of an image shown full screen: a scale over its fitted size and an offset, and where
// that puts the image, all kept pure so that zooming around the cursor is tested in zoom.test.ts.

/** `scale` 1 is the image fitted to the window; `x`, `y` move it from the centre, in pixels. */
export type Zoom = { scale: number; x: number; y: number };

export const FITTED: Zoom = { scale: 1, x: 0, y: 0 };
export const MAX_SCALE = 10;

/**
 * Scales by `factor` keeping the image point under `at` (relative to the centre of the fitted
 * image) where it is. Back at the fitted size the image is centred again.
 */
export function zoomAt(zoom: Zoom, factor: number, at: { x: number; y: number } = { x: 0, y: 0 }): Zoom {
  const scale = Math.min(MAX_SCALE, Math.max(1, zoom.scale * factor));
  if (scale === 1) return FITTED;
  const ratio = scale / zoom.scale;
  return { scale, x: at.x - (at.x - zoom.x) * ratio, y: at.y - (at.y - zoom.y) * ratio };
}

/** Moves a zoomed image; a fitted one stays put. */
export function pan(zoom: Zoom, dx: number, dy: number): Zoom {
  return zoom.scale === 1 ? zoom : { ...zoom, x: zoom.x + dx, y: zoom.y + dy };
}

/** A wheel or pinch's zoom factor: a trackpad pinch sends small deltas, a mouse wheel large ones. */
export function wheelFactor(deltaY: number): number {
  return Math.exp(-Math.max(-25, Math.min(25, deltaY)) * 0.01);
}

export type Size = { width: number; height: number };

/** The image's size fitted to the area, as `max-width` and `max-height` would: shrunk, never enlarged. */
export function fitSize(natural: Size, area: Size): Size {
  const ratio = Math.min(1, area.width / natural.width, area.height / natural.height);
  return { width: natural.width * ratio, height: natural.height * ratio };
}

/** Where a zoomed image shows: `x`, `y` its top-left corner from the area's centre, and its size. */
export type Placement = { x: number; y: number; width: number; height: number };

/**
 * Where a zoomed image shows. Its corner is rounded to device pixels, so that a pan moves it by
 * whole pixels instead of drawing it anew at another sub-pixel offset each frame.
 */
export function placement(zoom: Zoom, fitted: Size, pixelRatio = 1): Placement {
  const snap = (value: number) => Math.round(value * pixelRatio) / pixelRatio;
  const width = fitted.width * zoom.scale;
  const height = fitted.height * zoom.scale;
  return { x: snap(zoom.x - width / 2), y: snap(zoom.y - height / 2), width, height };
}

/**
 * A zoom `t` of the way from `from` to `to`, for an animated step. Interpolating the scale and the
 * offset linearly keeps fixed the point a `zoomAt` step keeps fixed, all along the way.
 */
export function between(from: Zoom, to: Zoom, t: number): Zoom {
  const mix = (a: number, b: number) => a + (b - a) * t;
  return { scale: mix(from.scale, to.scale), x: mix(from.x, to.x), y: mix(from.y, to.y) };
}

/** The image `delta` steps from `index` among `count` shown together: it stops at the first and the last. */
export function galleryStep(index: number, delta: number, count: number): number {
  return Math.min(count - 1, Math.max(0, index + delta));
}

/** An image of a set shown together. */
export type GalleryImage = { src: string | null; name: string };

/**
 * Where the image shown at `index` is once the set changed under it: the same image (the one
 * nearest `index` when it shows twice), else the image now at `index`, the last one when the set
 * shrank below it. Null once the set is empty.
 */
export function followImage(images: readonly GalleryImage[], shown: GalleryImage, index: number): number | null {
  if (images.length === 0) return null;
  const same = (image: GalleryImage) => image.src === shown.src && image.name === shown.name;
  let found: number | null = null;
  for (const [at, image] of images.entries()) {
    if (same(image) && (found === null || Math.abs(at - index) < Math.abs(found - index))) found = at;
  }
  return found ?? Math.min(index, images.length - 1);
}
