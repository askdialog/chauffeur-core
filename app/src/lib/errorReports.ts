// Which daemon failure an error toast shows, and what became of its report, for the app's error
// reports (docs/adr/error-reports-to-sentry.md).

import type { ErrorReportOutcome } from "@chauffeur/protocol";

/** How long after a failed call its toast may still come. */
const FAILURE_WINDOW_MS = 10_000;
const KEPT_FAILURES = 20;

export type Failure = { failure: string; message: string; at: number };

/** Keeps a failed call's failure, the oldest going past 20. */
export function keepFailure(failures: Failure[], failure: Failure) {
  failures.push(failure);
  if (failures.length > KEPT_FAILURES) failures.shift();
}

/** The failure a toast shows: the latest of the last ten seconds whose message the toast carries,
 * as call sites write it (`Could not pin it: ${errorMessage(error)}`). */
export function failureOf(failures: Failure[], message: string, now: number): string | undefined {
  for (let i = failures.length - 1; i >= 0; i--) {
    const f = failures[i];
    if (now - f.at > FAILURE_WINDOW_MS) break;
    if (f.message && message.includes(f.message)) return f.failure;
  }
  return undefined;
}

export function reportNote(outcome: ErrorReportOutcome): string {
  switch (outcome) {
    case "sent":
      return "Report sent";
    case "queued":
      return "Report kept: sent once Sentry is reachable";
    case "duplicate":
      return "Already reported a few minutes ago";
    case "limited":
      return "Not sent: today's reports are spent";
  }
}
