import { afterEach, beforeEach, describe, expect, jest, test } from "bun:test";
import { holdAudio, REST_MS } from "./audioHold";

const context = () => {
  const audio = { suspended: 0, suspend: async () => void (audio.suspended += 1) };
  return audio;
};

describe("audio held while in use", () => {
  beforeEach(() => jest.useFakeTimers());
  afterEach(() => jest.useRealTimers());

  test("a context rests suspended once nothing holds it", () => {
    const audio = context();
    holdAudio(audio)();
    expect(audio.suspended).toBe(0);
    jest.advanceTimersByTime(REST_MS);
    expect(audio.suspended).toBe(1);
  });

  test("it keeps running while another hold is taken, and a hold taken meanwhile cancels the rest", () => {
    const audio = context();
    const sound = holdAudio(audio);
    const square = holdAudio(audio);
    sound();
    sound();
    jest.advanceTimersByTime(REST_MS * 4);
    expect(audio.suspended).toBe(0);
    square();
    const next = holdAudio(audio);
    jest.advanceTimersByTime(REST_MS * 4);
    expect(audio.suspended).toBe(0);
    next();
    jest.advanceTimersByTime(REST_MS);
    expect(audio.suspended).toBe(1);
  });
});
