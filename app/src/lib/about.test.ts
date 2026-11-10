import { describe, expect, test } from "bun:test";
import { daemonMismatch, updateNote } from "./about";

describe("daemonMismatch", () => {
  test("a daemon of the app's version says nothing", () => {
    expect(daemonMismatch("0.16.0", "0.16.0")).toBeNull();
  });

  test("a daemon not connected yet says nothing", () => {
    expect(daemonMismatch("0.16.0", undefined)).toBeNull();
  });

  test("a daemon of another version is named, with how to restart it", () => {
    expect(daemonMismatch("0.16.0", "0.15.0")).toBe(
      'The daemon runs 0.15.0: restart it to run 0.16.0 ("chauffeur daemon stop", then reopen Chauffeur).',
    );
  });
});

describe("updateNote", () => {
  test("nothing published, or no answer from the daemon, says nothing", () => {
    expect(updateNote(null)).toBeNull();
    expect(updateNote({ current: "0.16.0", available: null, phase: "idle", error: null, releases_url: null })).toBeNull();
  });

  test("a newer version points at the sidebar's update card", () => {
    expect(updateNote({ current: "0.16.0", available: "0.17.0", phase: "idle", error: null, releases_url: null })).toBe(
      "Chauffeur 0.17.0 is available: update it from the card at the foot of the sidebar.",
    );
  });
});
