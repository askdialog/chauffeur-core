// How fast a plan's usage window is used, as tokens-on-track tells it: the share used against the
// share of the window gone by; and the words of the usage panel (names, resets, projections,
// extra usage), in the user's locale.

import type { AgentUsage, ExtraUsage, UsageWindow } from "@chauffeur/protocol";

export type Tone = "ok" | "warn" | "danger";

/** The locale and time zone dates are written in: the user's, unless a test fixes them. */
export type DateFormat = { locale?: string; timeZone?: string };

/** Before this share of the window has gone by, the pace says nothing. */
const EARLY = 5;

/** `target`: the share of the window gone by (0–100); `ratio`: used ÷ target, once it means something. */
export function pace(window: UsageWindow, now = Date.now()): { tone: Tone; target: number | null; ratio: number | null } {
  const used = window.used_percent;
  const reset = window.resets_at ? Date.parse(window.resets_at) : NaN;
  const length = window.window_secs;
  const target =
    length && Number.isFinite(reset) ? Math.min(100, Math.max(0, ((length - (reset - now) / 1000) / length) * 100)) : null;
  // Too early in the window, the ratio says nothing.
  const ratio = target != null && target >= EARLY ? used / target : null;
  if (used >= 90 || (ratio != null && ratio > 1.5)) return { tone: "danger", target, ratio };
  if (ratio != null) return { tone: ratio > 1 ? "warn" : "ok", target, ratio };
  return { tone: used >= 60 ? "danger" : used >= 30 ? "warn" : "ok", target, ratio };
}

/** `1h12`, `5d 20h`, `12 min`: the time left before a window resets. */
export function resetsIn(resetsAt: string | null, now = Date.now()): string | null {
  if (!resetsAt) return null;
  const minutes = Math.max(0, Math.round((Date.parse(resetsAt) - now) / 60_000));
  if (!Number.isFinite(minutes)) return null;
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h${String(minutes % 60).padStart(2, "0")}`;
  return `${Math.floor(hours / 24)}d ${hours % 24}h`;
}

/**
 * A window's name in the panel: `5h`, `7d`, and the weekly model-scoped ones by their model alone
 * (`7d Fable` → `Fable`), unless the same name has another window too (`Spark 5h`, `Spark 7d`).
 */
export function windowName(label: string, labels: string[] = [label]): string {
  const [span, ...rest] = label.split(" ");
  const name = rest.join(" ");
  if (!name) return label;
  const spans = labels.filter((other) => other.split(" ").slice(1).join(" ") === name).length;
  return span === "7d" && spans <= 1 ? name : `${name} ${span}`;
}

const PLANS: Record<string, string> = { prolite: "Pro Lite", promax: "Pro Max", edu_plus: "Edu Plus", edu_pro: "Edu Pro" };

/** `max` → `Max`, `prolite` → `Pro Lite`. */
export function planName(plan: string): string {
  return PLANS[plan] ?? plan.replace(/_/g, " ").replace(/\b\w/g, (c) => c.toUpperCase());
}

/** The day of `at` in the time zone, as a count of days, to tell today from tomorrow. */
function dayNumber(at: number, format: DateFormat): number {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone: format.timeZone,
    year: "numeric",
    month: "numeric",
    day: "numeric",
  }).formatToParts(at);
  const get = (type: string) => Number(parts.find((p) => p.type === type)?.value);
  return Date.UTC(get("year"), get("month") - 1, get("day")) / 86_400_000;
}

/** `16:50` on a 24-hour clock, `4:50 PM` on a 12-hour one. */
function timeFormat(format: DateFormat): Intl.DateTimeFormat {
  const options = { timeZone: format.timeZone, minute: "2-digit" } as const;
  const twelve = new Intl.DateTimeFormat(format.locale, { ...options, hour: "numeric" }).resolvedOptions().hour12;
  return new Intl.DateTimeFormat(format.locale, { ...options, hour: twelve ? "numeric" : "2-digit" });
}

/**
 * When `at` is, in words: `16:50` today, `Tue 13:00` within a week either way, `24 Oct 13:00` further
 * (12- or 24-hour as the locale writes it).
 */
export function clockParts(at: number, now = Date.now(), format: DateFormat = {}): { day: string | null; time: string } {
  // To the nearest minute, as the time left is (resets fall at 16:49:59.8).
  at = Math.round(at / 60_000) * 60_000;
  const time = timeFormat(format).format(at);
  const days = dayNumber(at, format) - dayNumber(now, format);
  if (days === 0) return { day: null, time };
  const day = new Intl.DateTimeFormat(
    format.locale,
    Math.abs(days) < 7 ? { timeZone: format.timeZone, weekday: "short" } : { timeZone: format.timeZone, day: "numeric", month: "short" },
  ).format(at);
  return { day, time };
}

export function clockTime(at: number, now = Date.now(), format: DateFormat = {}): string {
  const { day, time } = clockParts(at, now, format);
  return day ? `${day} ${time}` : time;
}

/** `in 2h15 · 16:50`, `in 3d 18h · Tue 13:00`. */
export function resetText(resetsAt: string | null, now = Date.now(), format: DateFormat = {}): string | null {
  const left = resetsIn(resetsAt, now);
  if (!resetsAt || !left) return null;
  return `in ${left} · ${clockTime(Date.parse(resetsAt), now, format)}`;
}

/**
 * When the window would be used up at the rate it has been used so far, if that comes before it
 * resets; null too early in the window (the first 5%), when nothing is used or it is used up.
 */
export function runsOutAt(window: UsageWindow, now = Date.now()): number | null {
  const { target } = pace(window, now);
  const used = window.used_percent;
  if (target == null || target < EARLY || used <= 0 || used >= 100 || !window.resets_at || !window.window_secs) return null;
  const reset = Date.parse(window.resets_at);
  const start = reset - window.window_secs * 1000;
  const at = start + ((now - start) * 100) / used;
  return at < reset ? at : null;
}

/** `At this pace, 7d runs out Mon ~14:00`; `… 5h runs out in 1h10 (~17:20)` the same day. */
export function projectionText(window: UsageWindow, name: string, now = Date.now(), format: DateFormat = {}): string | null {
  const at = runsOutAt(window, now);
  if (at == null) return null;
  // To the nearest 10 minutes: a projection, not a promise.
  const rounded = Math.round(at / 600_000) * 600_000;
  const { day, time } = clockParts(rounded, now, format);
  const when = day ? `${day} ~${time}` : `in ${resetsIn(new Date(at).toISOString(), now)} (~${time})`;
  return `At this pace, ${name} runs out ${when}`;
}

/** An amount in minor units (cents) of the currency, written as the locale writes money. */
export function money(minor: number, currency: string, locale?: string): string {
  try {
    const digits = new Intl.NumberFormat(locale, { style: "currency", currency }).resolvedOptions().maximumFractionDigits ?? 2;
    const value = minor / 10 ** digits;
    const whole = Number.isInteger(value);
    return new Intl.NumberFormat(locale, {
      style: "currency",
      currency,
      minimumFractionDigits: whole ? 0 : digits,
      maximumFractionDigits: whole ? 0 : digits,
    }).format(value);
  } catch {
    // Not a currency Intl knows.
    return `${(minor / 100).toFixed(2)} ${currency}`;
  }
}

/**
 * `Extra usage: $12.40 of $50 this month`; null when the plan has none turned on and nothing was
 * spent. Turned off by its limit: `· limit reached`; otherwise `· turned off`.
 */
export function extraUsageText(extra: ExtraUsage | null | undefined, locale?: string): string | null {
  if (!extra || (!extra.enabled && !extra.used)) return null;
  const currency = extra.currency || "USD";
  const used = extra.used ?? 0;
  let text = `Extra usage: ${money(used, currency, locale)}`;
  if (extra.limit != null) text += ` of ${money(extra.limit, currency, locale)}`;
  text += " this month";
  if (!extra.enabled) text += extra.limit != null && used >= extra.limit ? " · limit reached" : " · turned off";
  return text;
}

/** `read at 21:04`, `read Tue 21:04`. */
export function readAtText(readAt: string | null, now = Date.now(), format: DateFormat = {}): string | null {
  const at = readAt ? Date.parse(readAt) : NaN;
  if (!Number.isFinite(at)) return null;
  const { day, time } = clockParts(at, now, format);
  return day ? `read ${day} ${time}` : `read at ${time}`;
}

/** `Last read failed at 21:10; showing the reading from 21:04`; null when the last read worked. */
export function failureText(usage: AgentUsage, now = Date.now(), format: DateFormat = {}): string | null {
  if (!usage.error) return null;
  const at = (iso: string | null | undefined) => {
    const ms = iso ? Date.parse(iso) : NaN;
    return Number.isFinite(ms) ? clockTime(ms, now, format) : null;
  };
  const failed = at(usage.failed_at);
  const read = at(usage.read_at);
  const head = failed ? `Last read failed at ${failed}` : "Last read failed";
  return read ? `${head}; showing the reading from ${read}` : head;
}
