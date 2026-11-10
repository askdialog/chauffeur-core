import { expect, test } from "bun:test";
import { usageNotice } from "./usage-notice";

const format = { locale: "fr-FR", timeZone: "Europe/Paris" };
test("renders the persisted weekly warning in local time", () => {
  const notice = usageNotice("Claude usage limit nearly reached (seven_day), resets at 2026-10-21T11:00:00.000Z", format);
  expect(notice?.title).toBe("Limite hebdomadaire Claude presque atteinte");
  expect(notice?.reset).toContain("21 octobre 2026");
  expect(notice?.reset).toContain("13:00");
});
test("handles reached, unknown windows and absent or invalid resets", () => {
  expect(usageNotice("Claude usage limit reached (five_hour)", format)).toEqual({ title: "Limite de session Claude atteinte", reset: null });
  expect(usageNotice("Claude usage limit nearly reached (future), resets at invalid", { locale: "en-GB" })).toEqual({ title: "Claude usage limit nearly reached", reset: null });
  expect(usageNotice("Claude usage limit reached (seven_day_opus)", { locale: "en-US" })?.title).toBe("Claude weekly Opus usage limit reached");
});
test("leaves other notices alone", () => {
  expect(usageNotice("Chauffeur restarted", format)).toBeNull();
});
