import { describe, expect, test } from "bun:test";
import type {
  ReviewEvidence,
  ReviewExperiment,
  ReviewFollowUp,
  ReviewPattern,
  ReviewReport,
  ReviewSourceCoverage,
  ReviewsInfo,
} from "@chauffeur/protocol";
import {
  EMPTY_DRAFT,
  FollowUpDrafts,
  SnapshotGate,
  autoRefreshKey,
  coverageSummary,
  distinctObservations,
  draftStatus,
  effectiveStatus,
  evolution,
  evolutionText,
  experimentsByAttention,
  followUpProblem,
  patternStats,
  shownExperiment,
  sourceChoices,
  weeksWithoutReports,
} from "./reviews";

// Local times, so the day of each observation does not depend on the machine's time zone.
const at = (month: number, day: number, hour = 12) => new Date(2026, month - 1, day, hour).toISOString();

const evidence = (more: Partial<ReviewEvidence> = {}): ReviewEvidence => ({
  id: "e",
  workspace_id: "w1",
  chat_id: "c1",
  workspace: "atlas",
  branch: "main",
  at: at(11, 2),
  quote: "",
  source_seq: 1,
  from_review: false,
  role: "observation",
  human: false,
  ...more,
});

const pattern = (observations: ReviewEvidence[]): ReviewPattern => ({
  id: "p",
  title: "",
  summary: "",
  trend: "recurring",
  confidence: "",
  observations,
  counterexamples: [],
  gaps: [],
  report_ids: [],
});

const experiment = (more: Partial<ReviewExperiment> = {}): ReviewExperiment => ({
  id: "x",
  title: "",
  origin_report_id: "r",
  origin_date: "2026-11-08",
  pattern_id: null,
  experiment: "",
  expected_benefit: "",
  success_measure: "",
  effort: null,
  next_check: null,
  status: "proposed",
  evidence: [],
  ...more,
});

const followUp = (more: Partial<ReviewFollowUp> = {}): ReviewFollowUp => ({
  id: "f",
  experiment_id: "x",
  status: "decided",
  note: "",
  source: null,
  recorded_at: at(11, 9),
  ...more,
});

const report = (date: string, more: Partial<ReviewReport> = {}): ReviewReport => ({
  id: date,
  kind: "daily",
  date,
  workspace_id: "rw",
  chat_id: "rc",
  title: "Daily review",
  status: "complete",
  archived: false,
  shortened: false,
  summary: null,
  ...more,
});

const info = (more: Partial<ReviewsInfo> = {}): ReviewsInfo => ({
  source_seq: 1,
  auto_refresh_enabled: true,
  projection: null,
  reports: [report("2026-11-08")],
  followups: [],
  running: false,
  stale: true,
  last_error: null,
  coverage_start: "2026-10-12",
  coverage_end: "2026-11-08",
  next_weekly: null,
  ...more,
});

describe("reviews", () => {
  test("a poll cannot cross an in-flight user save or overwrite its result", () => {
    const gate = new SnapshotGate();
    const poll = gate.issue();
    const save = gate.beginMutation();
    expect(gate.mutating).toBe(true);
    expect(gate.accept(poll)).toBe(false);
    expect(gate.accept(save)).toBe(true);
    gate.endMutation();
    expect(gate.mutating).toBe(false);
    expect(gate.accept(gate.issue())).toBe(true);
  });

  test("equal follow-up timestamps keep the daemon's chronological save order", () => {
    const earlier = followUp({id: "z", status: "decided"});
    const later = followUp({id: "a", status: "tried"});
    expect(effectiveStatus(experiment(), [earlier, later])).toBe("tried");
  });

  test("new text in the same report invalidates a previous automatic refresh attempt", () => {
    expect(autoRefreshKey(info({source_seq: 2}))).not.toBe(autoRefreshKey(info({source_seq: 1})));
  });

  test("the rolling window can refresh on a new day without any newly finished turn", () => {
    expect(autoRefreshKey(info({coverage_end:"2026-11-15"}))).not.toBe(autoRefreshKey(info({coverage_end:"2026-11-14"})));
  });

  test("a report still being written does not start another automatic analysis until a turn completes", () => {
    const writing = info({ reports: [report("2026-11-08", { status: "incomplete" })] });
    const statusMoved = info({ reports: [report("2026-11-08", { status: "complete" })] });
    expect(autoRefreshKey(statusMoved)).toBe(autoRefreshKey(writing));
  });

  test("a report repeating a failure and a second quote of the same session that day are not new observations", () => {
    const kept = distinctObservations([
      evidence({ id: "a", source_seq: 1 }),
      evidence({ id: "b", source_seq: 2, at: at(11, 2, 16) }),
      evidence({ id: "c", chat_id: "c2", source_seq: 3 }),
      evidence({ id: "d", source_seq: 4, at: at(11, 3) }),
      evidence({ id: "e", source_seq: 5, at: at(11, 7), from_review: true }),
    ]);
    expect(kept.map((e) => e.id)).toEqual(["a", "c", "d"]);
  });

  test("an exploration's missing branch is unknown rather than a distinct branch", () => {
    expect(patternStats(pattern([evidence({branch:""})])).branches).toBe(0);
  });

  test("a pattern's stats count distinct days, branches and workspaces, and set report repeats apart", () => {
    const stats = patternStats(
      pattern([
        evidence({ at: at(10, 30) }),
        evidence({ chat_id: "c2", workspace_id: "w2", workspace: "beacon", at: at(11, 2) }),
        evidence({ chat_id: "c3", branch: "feature", at: at(11, 6) }),
        evidence({ from_review: true, at: at(11, 7) }),
      ]),
    );
    expect(stats).toMatchObject({ observations: 3, days: 3, branches: 3, workspaces: 2, repeats: 1 });
    expect(stats.firstSeen).toBe(at(10, 30));
    expect(stats.lastSeen).toBe(at(11, 6));
  });

  test("a period is zero only when every session was read and a report covers it; partial evidence is a lower bound or unknown", () => {
    const days = (from: number, count: number, more: Partial<ReviewSourceCoverage> = {}): ReviewSourceCoverage[] =>
      Array.from({ length: count }, (_, i) => {
        const date = new Date(2026, 9, from + i);
        const day = `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
        return { date: day, available: 2, included: 2, complete: true, ...more };
      });
    const coverage = [
      ...days(12, 7), // Oct 12–18: read in full, a report: an exact zero
      ...days(19, 7, { included: 1, complete: false }), // Oct 19–25: sampled, a report, nothing found
      ...days(26, 7, { included: 1, complete: false }), // Oct 26–Nov 1: sampled, one observation
      ...days(33, 7), // Nov 2–8: read in full, but no report
    ];
    const weeks = evolution(
      pattern([evidence({ at: at(10, 28) }), evidence({ chat_id: "c2", at: at(11, 4) }), evidence({ chat_id: "c3", at: at(11, 6) })]),
      [report("2026-10-14"), report("2026-10-21"), report("2026-10-30")],
      coverage,
      "2026-11-08",
    );
    expect(weeks).toEqual([
      { start: "2026-10-12", end: "2026-10-18", observations: 0, certainty: "exact" },
      { start: "2026-10-19", end: "2026-10-25", observations: null, certainty: "incomplete" },
      { start: "2026-10-26", end: "2026-11-01", observations: 1, certainty: "at_least" },
      { start: "2026-11-02", end: "2026-11-08", observations: 2, certainty: "at_least" },
    ]);
    expect(weeks.map(evolutionText)).toEqual(["0 observed", "Incomplete evidence", "≥ 1 observation", "≥ 2 observations"]);
  });

  test("a report alone never makes a period zero, and a period with neither report nor sessions read has no evidence", () => {
    const weeks = evolution(pattern([]), [report("2026-10-14")], [], "2026-11-08");
    expect(weeks.map((w) => w.certainty)).toEqual(["incomplete", "none", "none", "none"]);
    expect(weeks.every((w) => w.observations === null)).toBe(true);
    expect(evolutionText(weeks[1]!)).toBe("No evidence");
  });

  test("the coverage summary says how many events were read and which days were only sampled", () => {
    const summary = coverageSummary([
      { date: "2026-11-03", available: 5, included: 2, complete: false },
      { date: "2026-11-02", available: 3, included: 3, complete: true },
    ]);
    expect(summary).toEqual({ available: 8, included: 5, sampledDays: ["2026-11-03"], unknownDays:26, complete: false });
    expect(coverageSummary([]).complete).toBe(false);
  });

  test("seven-day periods without a usable report are listed for the coverage", () => {
    expect(weeksWithoutReports([report("2026-10-14"), report("2026-11-03")], "2026-10-12", "2026-11-08")).toEqual([
      "Oct 19–25",
      "Oct 26–Nov 1",
    ]);
  });

  test("the latest dated follow-up overrides the status the reports derived", () => {
    const x = experiment({ status: "tried" });
    expect(effectiveStatus(x, [])).toBe("tried");
    const later = followUp({ id: "2", status: "effect_unknown", recorded_at: at(11, 12) });
    const earlier = followUp({ id: "1", status: "decided", recorded_at: at(11, 9) });
    expect(effectiveStatus(x, [later, earlier, followUp({ experiment_id: "other", status: "observed_effect", recorded_at: at(11, 13) })])).toBe(
      "effect_unknown",
    );
  });

  test("experiments waiting on the user come before those with an observed effect", () => {
    const done = experiment({ id: "done", status: "observed_effect", origin_date: "2026-11-10" });
    const old = experiment({ id: "old", origin_date: "2026-11-01" });
    const recent = experiment({ id: "recent", origin_date: "2026-11-08" });
    expect(experimentsByAttention([done, old, recent], []).map((e) => e.id)).toEqual(["recent", "old", "done"]);
  });

  test("insights refresh by themselves only for stale reports, with no run in progress or failed", () => {
    expect(autoRefreshKey(info())).not.toBeNull();
    expect(autoRefreshKey(info({ reports: [] }))).toBeNull();
    expect(autoRefreshKey(info({ stale: false }))).toBeNull();
    expect(autoRefreshKey(info({ running: true }))).toBeNull();
    expect(autoRefreshKey(info({ auto_refresh_enabled: false }))).toBeNull();
    expect(autoRefreshKey(info({ last_error: "model unavailable" }))).toBeNull();
  });

  test("an answer asked before one already applied never replaces it", () => {
    const gate = new SnapshotGate();
    const poll = gate.issue();
    const save = gate.issue();
    expect(gate.accept(save)).toBe(true);
    expect(gate.accept(poll)).toBe(false);
    expect(gate.isLatest(poll)).toBe(false);
    const next = gate.issue();
    expect(gate.isLatest(next)).toBe(true);
    expect(gate.accept(next)).toBe(true);
  });

  test("an observed effect needs a note with its measurement and date, and a source conversation", () => {
    expect(followUpProblem(EMPTY_DRAFT)).toContain("note");
    expect(followUpProblem({ choice: "decided", note: "ok", sourceSeq: null })).toContain("10–6000");
    expect(followUpProblem({ choice: "decided", note: "x".repeat(6001), sourceSeq: null })).toContain("10–6000");
    expect(followUpProblem({ choice: "decided", note: "I'll try it", sourceSeq: null })).toBeNull();
    expect(followUpProblem({ choice: "observed_effect", note: "It worked well on every review", sourceSeq: null })).toContain("source");
    expect(followUpProblem({ choice: "observed_effect", note: "It worked well", sourceSeq: 4 })).toContain("measurement");
    expect(followUpProblem({ choice: "observed_effect", note: "1 CI rework over 3 reviews, Nov 9–12", sourceSeq: 4 })).toBeNull();
  });

  test("a note-only follow-up records no status, and needs neither a source nor a measurement", () => {
    const note = { choice: "keep" as const, note: "Deferred to next week", sourceSeq: null };
    expect(draftStatus(note)).toBeNull();
    expect(draftStatus({ choice: "decided", note: "Yes, starting Monday", sourceSeq: null })).toBe("decided");
    expect(followUpProblem(note)).toBeNull();
  });

  test("a note never adopts or pins the status the reports derived", () => {
    const x = experiment({ status: "tried" });
    const note = followUp({ id: "n", status: null, note: "Will look again Friday", recorded_at: at(11, 10) });
    expect(effectiveStatus(x, [note])).toBe("tried");
    // The reports later derive another status: the earlier note does not hold the old one.
    expect(effectiveStatus({ ...x, status: "effect_unknown" }, [note])).toBe("effect_unknown");
  });

  test("a note after an explicit observed effect keeps that effect", () => {
    const observed = followUp({ id: "o", status: "observed_effect", recorded_at: at(11, 11) });
    const note = followUp({ id: "n", status: null, recorded_at: at(11, 12) });
    expect(effectiveStatus(experiment({ status: "proposed" }), [note, observed])).toBe("observed_effect");
  });

  test("a retry of the same draft reuses its idempotency key and status, a changed draft gets new ones", () => {
    const drafts = new FollowUpDrafts();
    let n = 0;
    const next = () => `k${++n}`;
    drafts.edit("x", { choice: "decided", note: "Yes, starting Monday", sourceSeq: null });
    const first = drafts.submit("x", next);
    expect(first).toMatchObject({ key: "k1", status: "decided" });
    drafts.failed("x", "daemon unreachable");
    expect(drafts.failure("x")).toBe("daemon unreachable");
    expect(drafts.draft("x").note).toBe("Yes, starting Monday");
    expect(drafts.submit("x", next)).toEqual(first);
    drafts.edit("x", { choice: "keep", note: "Yes, starting Tuesday", sourceSeq: null });
    expect(drafts.submit("x", next)).toMatchObject({ key: "k2", status: null });
  });

  test("drafts are kept per experiment, and a saved one is forgotten unless edited during the save", () => {
    const drafts = new FollowUpDrafts();
    drafts.edit("x", { ...EMPTY_DRAFT, note: "Draft for x, keep me" });
    drafts.edit("y", { ...EMPTY_DRAFT, note: "Draft for y" });
    expect(drafts.draft("x").note).toBe("Draft for x, keep me");
    const sent = drafts.submit("y", () => "k");
    drafts.saved("y", sent);
    expect(drafts.draft("y")).toEqual(EMPTY_DRAFT);
    const again = drafts.submit("x", () => "k2");
    drafts.edit("x", { ...EMPTY_DRAFT, note: "Typed more while saving" });
    drafts.saved("x", again);
    expect(drafts.draft("x").note).toBe("Typed more while saving");
  });

  test("the experiment shown stays the pinned one when newer data reorders the list", () => {
    const a = experiment({ id: "a", origin_date: "2026-11-01" });
    const b = experiment({ id: "b", origin_date: "2026-11-08" });
    expect(shownExperiment([b, a], "a")?.id).toBe("a");
    expect(shownExperiment([a, b], "a")?.id).toBe("a");
    expect(shownExperiment([b, a], "gone")?.id).toBe("b");
    expect(shownExperiment([], null)).toBeNull();
  });

  test("an update may cite the experiment's evidence and its pattern's original observations, each once", () => {
    const x = experiment({ evidence: [evidence({ source_seq: 7 })] });
    const p = pattern([evidence({ source_seq: 7 }), evidence({ source_seq: 8, chat_id: "c2" }), evidence({ source_seq: 9, from_review: true, chat_id: "c3" })]);
    expect(sourceChoices(x, p).map((e) => e.source_seq)).toEqual([7, 8]);
  });
});
