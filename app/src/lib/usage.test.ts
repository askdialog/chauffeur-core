import { describe, expect, test } from "bun:test";
import type { AgentUsage, UsageWindow } from "@chauffeur/protocol";
import {
  clockTime,
  extraUsageText,
  failureText,
  money,
  pace,
  planName,
  projectionText,
  readAtText,
  resetsIn,
  resetText,
  runsOutAt,
  windowName,
} from "./usage";

// Thursday 15 October 2026, noon UTC.
const now = Date.parse("2026-10-15T12:00:00Z");
const hours = (h: number) => new Date(now + h * 3_600_000).toISOString();
const window = (used: number, resetsInHours: number, length = 5 * 3600): UsageWindow => ({
  label: "5h",
  used_percent: used,
  resets_at: hours(resetsInHours),
  window_secs: length,
});
const WEEK = 7 * 24 * 3600;
const gb = { locale: "en-GB", timeZone: "UTC" };

describe("pace", () => {
  test("on pace, ahead of it, far ahead", () => {
    // 3 of 5 hours gone by: 60% of the window.
    expect(pace(window(50, 2), now)).toMatchObject({ tone: "ok", target: 60 });
    expect(pace(window(70, 2), now).tone).toBe("warn");
    expect(pace(window(95, 2), now).tone).toBe("danger");
    // 1 of 5 hours: 20% gone by, 40% used is twice the pace.
    expect(pace(window(40, 4), now).tone).toBe("danger");
  });

  test("too early in the window, the share used alone decides", () => {
    expect(pace(window(10, 4.9), now)).toMatchObject({ tone: "ok", ratio: null });
    expect(pace(window(35, 4.9), now).tone).toBe("warn");
    expect(pace({ ...window(65, 1), resets_at: null }, now).tone).toBe("danger");
  });

  test("time left", () => {
    expect(resetsIn(hours(0.2), now)).toBe("12 min");
    expect(resetsIn(hours(1.2), now)).toBe("1h12");
    expect(resetsIn(hours(140), now)).toBe("5d 20h");
  });
});

describe("the panel's words", () => {
  test("window names drop the weekly prefix of model-scoped windows", () => {
    const claude = ["5h", "7d", "7d Fable", "7d Opus"];
    expect(claude.map((label) => windowName(label, claude))).toEqual(["5h", "7d", "Fable", "Opus"]);
    // A name with two windows keeps their spans.
    const codex = ["5h", "7d", "5h Spark", "7d Spark"];
    expect(codex.map((label) => windowName(label, codex))).toEqual(["5h", "7d", "Spark 5h", "Spark 7d"]);
    expect(windowName("1d Mini")).toBe("Mini 1d");
    expect(windowName("limit")).toBe("limit");
  });

  test("plan names", () => {
    expect(planName("max")).toBe("Max");
    expect(planName("prolite")).toBe("Pro Lite");
    expect(planName("self_serve_business_prolite")).toBe("Self Serve Business Prolite");
  });

  test("resets, relative and on the clock: today, this week, further", () => {
    expect(resetText(hours(4.83), now, gb)).toBe("in 4h50 · 16:50");
    // Tuesday 20 October.
    expect(resetText(hours(5 * 24 + 1), now, gb)).toBe("in 5d 1h · Tue 13:00");
    expect(clockTime(Date.parse(hours(10 * 24)), now, gb)).toBe("25 Oct 12:00");
    expect(resetText(null, now, gb)).toBeNull();
    // The locale's own clock.
    expect(clockTime(Date.parse(hours(4.83)), now, { locale: "en-US", timeZone: "UTC" })).toBe("4:50 PM");
    // The day is the time zone's: 23:30 UTC is already Friday in Paris.
    expect(clockTime(Date.parse(hours(11.5)), now, { locale: "en-GB", timeZone: "Europe/Paris" })).toBe("Fri 01:30");
  });

  test("when it was read", () => {
    expect(readAtText(hours(-0.5), now, gb)).toBe("read at 11:30");
    expect(readAtText(hours(-24), now, gb)).toBe("read Wed 12:00");
    expect(readAtText(null, now, gb)).toBeNull();
  });
});

describe("projection", () => {
  test("runs out before the reset only when ahead of pace", () => {
    // 3 of 5 hours gone (60%), 75% used: 100% at 4h from the start, an hour from now.
    expect(runsOutAt(window(75, 2), now)).toBe(now + 3_600_000);
    expect(projectionText(window(75, 2), "5h", now, gb)).toBe("At this pace, 5h runs out in 1h00 (~13:00)");
    // On or behind pace: it lasts.
    expect(runsOutAt(window(60, 2), now)).toBeNull();
    expect(runsOutAt(window(30, 2), now)).toBeNull();
    expect(projectionText(window(30, 2), "5h", now, gb)).toBeNull();
  });

  test("a weekly window running out on another day", () => {
    // 2 of 7 days gone (28.6%), 40% used: out at 5 days from the start, Sunday 18 at 12:00.
    const weekly = { ...window(40, 5 * 24, WEEK), label: "7d" };
    expect(runsOutAt(weekly, now)).toBe(now + 3 * 24 * 3_600_000);
    expect(projectionText(weekly, "7d", now, gb)).toBe("At this pace, 7d runs out Sun ~12:00");
  });

  test("nothing too early in the window, with nothing used, used up or no reset", () => {
    // 3% of the window gone by.
    expect(runsOutAt(window(20, 4.85), now)).toBeNull();
    expect(runsOutAt(window(0, 2), now)).toBeNull();
    expect(runsOutAt(window(100, 2), now)).toBeNull();
    expect(runsOutAt({ ...window(75, 2), resets_at: null }, now)).toBeNull();
  });
});

describe("extra usage", () => {
  test("amounts in minor units, written in the currency", () => {
    expect(money(1240, "USD", "en-US")).toBe("$12.40");
    expect(money(5000, "USD", "en-US")).toBe("$50");
    expect(money(1500, "JPY", "en-US")).toBe("¥1,500");
    expect(money(1240, "dollars", "en-US")).toBe("12.40 dollars");
  });

  test("the line, when turned on or spent", () => {
    const extra = (enabled: boolean, used: number | null, limit: number | null, currency: string | null = "USD") => ({
      enabled,
      used,
      limit,
      currency,
    });
    expect(extraUsageText(extra(true, 1240, 5000), "en-US")).toBe("Extra usage: $12.40 of $50 this month");
    expect(extraUsageText(extra(true, null, null, null), "en-US")).toBe("Extra usage: $0 this month");
    expect(extraUsageText(extra(false, 5000, 5000), "en-US")).toBe("Extra usage: $50 of $50 this month · limit reached");
    expect(extraUsageText(extra(false, 300, 5000), "en-US")).toBe("Extra usage: $3 of $50 this month · turned off");
    expect(extraUsageText(extra(false, 0, null), "en-US")).toBeNull();
    expect(extraUsageText(null, "en-US")).toBeNull();
  });
});

describe("a failed read", () => {
  const usage = (over: Partial<AgentUsage>): AgentUsage => ({
    agent: "codex",
    plan: "prolite",
    windows: [],
    error: null,
    read_at: hours(-1),
    failed_at: null,
    extra_usage: null,
    account: "signed_in",
    account_error: null,
    ...over,
  });

  test("says when it failed and which reading is shown", () => {
    expect(failureText(usage({ error: "logged out", failed_at: hours(-0.1) }), now, gb)).toBe(
      "Last read failed at 11:54; showing the reading from 11:00",
    );
    expect(failureText(usage({ error: "logged out" }), now, gb)).toBe("Last read failed; showing the reading from 11:00");
    expect(failureText(usage({}), now, gb)).toBeNull();
  });
});
