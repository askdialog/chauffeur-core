import { describe, expect, test } from "bun:test";
import { between, FITTED, fitSize, followImage, galleryStep, MAX_SCALE, pan, placement, wheelFactor, zoomAt } from "./zoom";

describe("zoomAt", () => {
  test("keeps the point under the cursor in place", () => {
    const at = { x: 100, y: -40 };
    const zoomed = zoomAt(FITTED, 2, at);
    expect(zoomed).toEqual({ scale: 2, x: -100, y: 40 });
    // The image point under the cursor, (at - offset) / scale, is the same before and after.
    expect((at.x - zoomed.x) / zoomed.scale).toBe(at.x);
    expect((at.y - zoomed.y) / zoomed.scale).toBe(at.y);
  });

  test("never goes below the fitted size, which is centred again, nor past the maximum", () => {
    const zoomed = zoomAt(FITTED, 3, { x: 50, y: 50 });
    expect(zoomAt(zoomed, 0.1, { x: 10, y: 10 })).toEqual(FITTED);
    expect(zoomAt(zoomed, 100).scale).toBe(MAX_SCALE);
  });

  test("a fitted image does not pan; a zoomed one does", () => {
    expect(pan(FITTED, 10, 10)).toEqual(FITTED);
    expect(pan({ scale: 2, x: 0, y: 0 }, 10, -5)).toEqual({ scale: 2, x: 10, y: -5 });
  });

  test("a mouse wheel notch zooms by a bounded step", () => {
    expect(wheelFactor(-1000)).toBeCloseTo(Math.exp(0.25));
    expect(wheelFactor(4)).toBeLessThan(1);
  });

  test("an image is fitted as max-width and max-height would: shrunk to the area, never enlarged", () => {
    expect(fitSize({ width: 1600, height: 1000 }, { width: 800, height: 800 })).toEqual({ width: 800, height: 500 });
    expect(fitSize({ width: 1000, height: 2000 }, { width: 800, height: 800 })).toEqual({ width: 400, height: 800 });
    expect(fitSize({ width: 300, height: 200 }, { width: 800, height: 800 })).toEqual({ width: 300, height: 200 });
  });

  test("a fitted image shows centred, a zoomed one at its zoomed size", () => {
    const fitted = { width: 800, height: 500 };
    expect(placement(FITTED, fitted)).toEqual({ x: -400, y: -250, width: 800, height: 500 });
    expect(placement({ scale: 3, x: 10, y: -20 }, fitted)).toEqual({ x: -1190, y: -770, width: 2400, height: 1500 });
  });

  test("the shown image keeps the point under the cursor in place, to a device pixel", () => {
    const fitted = { width: 1010.8125, height: 631.75 };
    const at = { x: 123.4, y: -56.7 };
    const pixelRatio = 2;
    // The point under the cursor, as a fraction of the image.
    const under = (zoom: { scale: number; x: number; y: number }) => {
      const box = placement(zoom, fitted, pixelRatio);
      return { x: (at.x - box.x) / box.width, y: (at.y - box.y) / box.height, box };
    };
    let zoom = FITTED;
    // The point under the cursor before any zoom, unrounded.
    const start = { x: (at.x + fitted.width / 2) / fitted.width, y: (at.y + fitted.height / 2) / fitted.height };
    for (let i = 0; i < 30; i++) {
      zoom = zoomAt(zoom, wheelFactor(-4), at);
      const now = under(zoom);
      // Within one device pixel of the zoomed image.
      expect(Math.abs(now.x - start.x) * now.box.width).toBeLessThanOrEqual(1 / pixelRatio);
      expect(Math.abs(now.y - start.y) * now.box.height).toBeLessThanOrEqual(1 / pixelRatio);
      // Its corner on whole device pixels.
      expect(Number.isInteger(now.box.x * pixelRatio)).toBe(true);
      expect(Number.isInteger(now.box.y * pixelRatio)).toBe(true);
    }
  });

  test("an animated step keeps the point under the cursor in place all along", () => {
    const at = { x: -210, y: 75 };
    const from = zoomAt(FITTED, 1.7, { x: 40, y: 40 });
    const to = zoomAt(from, 1.25, at);
    const before = { x: (at.x - from.x) / from.scale, y: (at.y - from.y) / from.scale };
    for (const t of [0, 0.1, 0.5, 0.9, 1]) {
      const zoom = between(from, to, t);
      expect((at.x - zoom.x) / zoom.scale).toBeCloseTo(before.x, 9);
      expect((at.y - zoom.y) / zoom.scale).toBeCloseTo(before.y, 9);
    }
    expect(between(from, to, 1)).toEqual(to);
  });
});

describe("galleryStep", () => {
  test("moves to the next or previous image, and stops at the first and the last", () => {
    expect(galleryStep(1, 1, 3)).toBe(2);
    expect(galleryStep(1, -1, 3)).toBe(0);
    expect(galleryStep(2, 1, 3)).toBe(2);
    expect(galleryStep(0, -1, 3)).toBe(0);
    expect(galleryStep(0, 1, 1)).toBe(0);
  });
});

describe("followImage", () => {
  const a = { src: "blob:a", name: "a.png" };
  const b = { src: "blob:b", name: "b.png" };
  const c = { src: "blob:c", name: "c.png" };

  test("an unchanged set keeps the image shown", () => {
    expect(followImage([a, b, c], b, 1)).toBe(1);
  });

  test("the image shown is followed when a preview arrives before it", () => {
    expect(followImage([c, a, b], b, 1)).toBe(2);
  });

  test("when the image shown is gone, the one now at its place shows, or the last", () => {
    expect(followImage([a, c], b, 1)).toBe(1);
    expect(followImage([a, b], c, 2)).toBe(1);
  });

  test("an image shown twice stays the one stepped to", () => {
    expect(followImage([a, b, a], a, 2)).toBe(2);
    expect(followImage([a, b, a], a, 0)).toBe(0);
  });

  test("an empty set shows nothing", () => {
    expect(followImage([], a, 0)).toBeNull();
  });
});
