# Reviews learn across days and follow their recommendations

- **Date:** 2026-11-14
- **Status:** accepted

## Context

Justin wants reviews to detect recurring patterns and recommend improvements beyond one day's
activity. Independent daily reports cannot establish trends or tell whether earlier advice helped.

## Decision

Decided with Justin 2026-11-14: daily reviews receive the previous seven calendar days of daily
reports and the latest weekly synthesis. They distinguish new findings, recurrences, improvements
and regressions, avoiding unchanged repeated advice. A weekly synthesis reads four weeks of
reports, groups observations into patterns and selects one to three concrete experiments with
observable success measures. Recommendations retain their original date, evidence, next check
and status: proposed, decided, tried, observed effect, or effect unknown. Only user decisions or
observed actions establish adoption and effects. Repeated summaries of one incident count once.

The implementation runs weekly reviews on Sunday at 18:00 local time, in a separate read-only
workspace in the existing Daily reviews section, using the daily review's enablement and model.
It catches up after sleep or restart, waits for ongoing daily analyses, skips inactive weeks,
and does not recreate archived or deleted reviews. First enablement starts with the current
week. History includes the review conversation's follow-up decisions, including archived reviews,
and identifies missing, incomplete or shortened evidence. No new interface or automatic action
on recommendations is introduced.

## Consequences

The daemon exports bounded snapshots from its stored conversations rather than relying on files
in old worktrees. A daily history includes at most seven days of daily reports and the most recent
weekly report within 28 days; a weekly history includes 28 calendar days of daily reports and prior
weekly syntheses within 28 days. Each conversation keeps its latest 200 relevant events within
128,000 text characters, with explicit truncation metadata. User messages, assistant messages and
turn outcomes are exported; tool outputs are excluded from report history. Raw sessions remain
available for the daily review's day and the weekly review's current week. History is a snapshot
at execution, not a reconstruction of what was known at the historical report date.

Both review kinds are excluded from session activity and routine discovery. Missed active days
wait for the preceding analyses before starting, so catch-up can use their finished reports.
Catch-up runs each Sunday's synthesis before the following daily reports, and waits for
starting or working analyses. An unanswered question or permission request in a previous
review does not block later reports; the snapshot retains that incomplete status.
Reviews remain analysis only: no code changes, tickets, external comments or agent delegation.
