# Reviews has an overview, evidence and dated recommendation follow-up

- **Date:** 2026-11-14
- **Status:** accepted

## Context

Justin approved a visual Reviews proposal after the cross-day daily and weekly analyses were
implemented. Reading separate report conversations makes recurring patterns and outstanding
recommendations hard to follow. The approved canvas is in the workspace's Design — Reviews chat.

## Decision

Asked by Justin and approved for implementation 2026-11-14: add Reviews to the existing Daily
reviews section, preserving its report conversations. Its Overview shows the synthesis,
recurring or improving patterns and up to three experiments. Pattern detail links original
observations, confidence, counterexamples and gaps. Experiments shows a recommendation's
origin, measurable success criterion and dated decisions/results. Report history opens the
original conversations, archived ones included, and exposes missing or shortened coverage.
The existing Daily review settings are accessible from Reviews and explain the weekly cadence
and history windows; no new schedule or window controls are introduced.

A user can record a decision, trial or result, with a note and optional source. An observed
effect requires a dated source and a measurement note. The daemon owns these durable writes,
retries save once, and a model refresh cannot overwrite user follow-up. A note-only update
records no status and never adopts or freezes a model-derived status. New daily and weekly
reviews receive the recorded follow-ups as historical data. Nothing posts to an external system.

## Consequences

Existing prose reports are projected into structured insights through a bounded, tool-free
call to the configured daily-review model. Opening Reviews refreshes stale insights when
scheduled reviews are enabled; a manual refresh is available even when the schedule is off.
Only completed report turns or a changed calendar window invalidate insights; streaming text
does not repeatedly start model calls. No model work runs just because the app is closed or a
hidden view receives an event. There
is at most one projection per daemon, and new or failed projections keep the prior insights
and source history accessible. Restarted or failed analyses can be retried.

The daemon supplies actual event identifiers, timestamps, workspace/chat references and
excerpts; invented references are rejected. Original observations are conservatively counted
once per chat and local day. Report repetitions never become observations. Counts describe
observations, not proven independent root incidents, and missing reports never imply success.
Original sources are sampled separately per local day and chat, so recent tool traffic cannot
displace earlier weeks. Per-day available/included event counts and incomplete source coverage
are explicit. A period with missing source coverage is unknown or a lower bound, never zero.
Source excerpts, report coverage and extraction limits are explicit. Unsupported adoption
claims are downgraded; the latest explicit user update overrides report-derived status.

The visual view uses real data, not the prototype's fictional examples or preview controls.
The latest 90 reports are listed; model input covers 28 days with bounded report/source
excerpts, compact prior recommendation identities and recent user follow-up. Recommendation
identities and saved follow-up remain durable across refresh and restart. Model carry-over is
bounded to 50 untracked unresolved recommendations; explicitly followed recommendations remain
accessible, while resolved untracked suggestions are left in their source reports. The view is verified
on a temporary daemon with deterministic model output and a separate headless browser.
