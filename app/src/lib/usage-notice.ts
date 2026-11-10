import type { DateFormat } from "./usage";

/** Recognizes the persisted Claude notices, including those from older sessions. */
export function usageNotice(message: string, format: DateFormat = {}): { title: string; reset: string | null } | null {
  const match = /^Claude usage limit (nearly reached|reached)(?: \(([^)]+)\))?(?:, resets at (\S+))?$/.exec(message);
  if (!match) return null;
  const locale = format.locale ?? navigator.language;
  const french = locale.startsWith("fr");
  const windows: Record<string, [string, string]> = {
    five_hour: ["session", "de session"],
    seven_day: ["weekly", "hebdomadaire"],
    seven_day_opus: ["weekly Opus", "hebdomadaire Opus"],
    seven_day_sonnet: ["weekly Sonnet", "hebdomadaire Sonnet"],
  };
  const window = match[2] ? windows[match[2]] : undefined;
  const qualifier = window ? (french ? window[1] : window[0]) : "";
  const title = french
    ? `Limite${qualifier ? ` ${qualifier}` : ""} Claude ${match[1] === "reached" ? "atteinte" : "presque atteinte"}`
    : `Claude${qualifier ? ` ${qualifier}` : ""} usage limit ${match[1]}`;
  const at = match[3] ? Date.parse(match[3]) : NaN;
  const date = Number.isFinite(at)
    ? new Intl.DateTimeFormat(locale, {
        timeZone: format.timeZone,
        day: "numeric", month: "long", year: "numeric", hour: "numeric", minute: "2-digit",
      }).format(at)
    : null;
  return { title, reset: date ? `${french ? "Réinitialisation le" : "Resets"} ${date}` : null };
}
