import { describe, expect, test } from "bun:test";
import { type Failure, failureOf, keepFailure, reportNote } from "./errorReports";

describe("failureOf", () => {
  test("finds the recent failure whose message the toast carries", () => {
    const failures: Failure[] = [];
    keepFailure(failures, { failure: "f-1", message: "disk full", at: 1_000 });
    keepFailure(failures, { failure: "f-2", message: "no such section", at: 2_000 });
    expect(failureOf(failures, "Could not move the section: no such section", 3_000)).toBe("f-2");
    expect(failureOf(failures, "Could not move it: disk full", 3_000)).toBe("f-1");
    expect(failureOf(failures, "Could not open the browser", 3_000)).toBeUndefined();
  });

  test("a failure older than ten seconds is not the toast's", () => {
    const failures: Failure[] = [{ failure: "f-3", message: "too late", at: 10_000 }];
    expect(failureOf(failures, "x: too late", 21_000)).toBeUndefined();
  });

  test("only the last twenty failures are kept", () => {
    const failures: Failure[] = [];
    for (let n = 0; n < 25; n++) keepFailure(failures, { failure: `f-${n}`, message: `error ${n}`, at: n });
    expect(failures).toHaveLength(20);
    expect(failures[0].failure).toBe("f-5");
  });
});

test("each outcome of a report says what became of it", () => {
  expect(reportNote("sent")).toBe("Report sent");
  expect(reportNote("queued")).toContain("once Sentry is reachable");
  expect(reportNote("duplicate")).toContain("Already reported");
  expect(reportNote("limited")).toContain("today's reports");
});
