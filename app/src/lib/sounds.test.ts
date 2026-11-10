import { afterAll, beforeAll, describe, expect, jest, test } from "bun:test";
import { BUILT_IN_SOUNDS, playSound } from "./sounds";

/** A context that only keeps its state: what the page's audio threads follow. */
class FakeContext {
  state: "running" | "suspended" = "suspended";
  currentTime = 0;
  destination = {};
  async resume() {
    this.state = "running";
  }
  async suspend() {
    this.state = "suspended";
  }
  createOscillator() {
    return { frequency: {}, connect: (node: unknown) => node, start() {}, stop() {} };
  }
  createGain() {
    const param = { setValueAtTime() {}, linearRampToValueAtTime() {}, exponentialRampToValueAtTime() {} };
    return { gain: param, connect: (node: unknown) => node };
  }
}

describe("notification sounds", () => {
  test("the app's own sounds are short and audible", () => {
    for (const { notes } of Object.values(BUILT_IN_SOUNDS)) {
      const end = Math.max(...notes.map(([, start, length]) => start + length));
      expect(end).toBeLessThan(2);
      for (const [frequency] of notes) expect(frequency).toBeGreaterThan(200);
    }
    expect(Object.keys(BUILT_IN_SOUNDS)).toEqual(["chime", "bell", "station"]);
  });

  test("no sound plays nothing (and needs no audio at all)", async () => {
    await playSound("", () => null);
    await playSound("none", () => null);
  });
});

describe("the page's audio between sounds", () => {
  const original = globalThis.AudioContext;
  let made: FakeContext | null = null;
  beforeAll(() => {
    (globalThis as { AudioContext: unknown }).AudioContext = class extends FakeContext {
      constructor() {
        super();
        made = this;
      }
    };
    jest.useFakeTimers();
  });
  afterAll(() => {
    jest.useRealTimers();
    (globalThis as { AudioContext: unknown }).AudioContext = original;
  });

  test("a sound keeps it running until it ended, then it rests until the next", async () => {
    const [, start, length] = BUILT_IN_SOUNDS.bell.notes[0];
    await playSound("bell", () => null);
    expect(made?.state).toBe("running");
    jest.advanceTimersByTime(((start + length) * 1000) / 2);
    expect(made?.state).toBe("running");
    jest.advanceTimersByTime(5000);
    expect(made?.state).toBe("suspended");
    await playSound("chime", () => null);
    expect(made?.state).toBe("running");
  });
});
