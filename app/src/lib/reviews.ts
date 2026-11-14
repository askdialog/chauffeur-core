// What the Reviews page derives from `reviews.info`: statuses with the user's follow-ups laid
// over the report's, conservatively counted observations, a four-week evolution where zero needs
// complete source coverage (otherwise unknown or a lower bound), the drafts of follow-ups, and
// which daemon answer may replace the page's data.
// It never invents data: everything is read from the projection, the reports and the follow-ups.
import type {
  ReviewEvidence,
  ReviewExperiment,
  ReviewFollowUp,
  ReviewPattern,
  ReviewRecommendationStatus,
  ReviewReport,
  ReviewReportStatus,
  ReviewSourceCoverage,
  ReviewsInfo,
  ReviewTrend,
} from "@chauffeur/protocol";

export type Tone = "neutral" | "accent" | "working" | "attention" | "danger" | "success";

const STATUS: Record<ReviewRecommendationStatus, { label: string; tone: Tone }> = {
  proposed: { label: "Proposed", tone: "neutral" },
  decided: { label: "Decided", tone: "accent" },
  tried: { label: "Tried", tone: "working" },
  observed_effect: { label: "Observed effect", tone: "success" },
  effect_unknown: { label: "Effect unknown", tone: "attention" },
};

export function statusLabel(status: ReviewRecommendationStatus): string {
  return STATUS[status].label;
}

export function statusTone(status: ReviewRecommendationStatus): Tone {
  return STATUS[status].tone;
}

export function trendLabel(trend: ReviewTrend): string {
  return trend === "recurring" ? "Recurring" : trend === "improving" ? "Improving" : "Unclear";
}

export function trendTone(trend: ReviewTrend): Tone {
  return trend === "recurring" ? "attention" : trend === "improving" ? "success" : "neutral";
}

export function reportStatusLabel(status: ReviewReportStatus): string {
  return { complete: "Complete", incomplete: "Incomplete", failed: "Failed", unavailable: "Unavailable" }[status];
}

export function reportStatusTone(status: ReviewReportStatus): Tone {
  return status === "complete" ? "neutral" : status === "failed" ? "danger" : "attention";
}

/** A follow-up's moment, for ordering: its recorded time; equal times preserve the daemon’s save order. */
function followUpOrder(a: ReviewFollowUp, b: ReviewFollowUp): number {
  return Date.parse(a.recorded_at) - Date.parse(b.recorded_at);
}

/** The experiment's follow-ups, oldest first. */
export function followUpsOf(experimentId: string, followups: ReviewFollowUp[]): ReviewFollowUp[] {
  return followups.filter((f) => f.experiment_id === experimentId).sort(followUpOrder);
}

/**
 * The status to show: the newest status the user recorded explicitly, else what the reports
 * derived. A note-only follow-up (`status: null`) neither adopts nor pins the shown status.
 */
export function effectiveStatus(experiment: ReviewExperiment, followups: ReviewFollowUp[]): ReviewRecommendationStatus {
  return followUpsOf(experiment.id, followups).findLast((f) => f.status !== null)?.status ?? experiment.status;
}

/** A local calendar day, `YYYY-MM-DD`, of an ISO time or of a date already in that form. */
export function localDay(value: string): string {
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) return value;
  const t = new Date(value);
  if (Number.isNaN(t.getTime())) return value.slice(0, 10);
  return `${t.getFullYear()}-${String(t.getMonth() + 1).padStart(2, "0")}-${String(t.getDate()).padStart(2, "0")}`;
}

function dayDate(day: string): Date {
  const [y, m, d] = localDay(day).split("-").map(Number);
  return new Date(y ?? 1970, (m ?? 1) - 1, d ?? 1);
}

/** `Nov 8`, from a `YYYY-MM-DD` day or an ISO time, in local time. */
export function shortDay(value: string): string {
  const date = dayDate(value);
  if (Number.isNaN(date.getTime())) return value;
  return date.toLocaleDateString("en-US", { month: "short", day: "numeric" });
}

/** `Oct 12–18`, or `Oct 26–Nov 1` across months. */
export function dayRange(from: string, to: string): string {
  const a = dayDate(from);
  const b = dayDate(to);
  if (a.getMonth() === b.getMonth() && a.getFullYear() === b.getFullYear()) return `${shortDay(from)}–${b.getDate()}`;
  return `${shortDay(from)}–${shortDay(to)}`;
}

/** `14:12`, the local time of an ISO time; empty for a bare date. */
export function localTime(value: string): string {
  if (/^\d{4}-\d{2}-\d{2}$/.test(value)) return "";
  const t = new Date(value);
  if (Number.isNaN(t.getTime())) return "";
  return t.toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit" });
}

/**
 * The original observations, one per chat and local day: a report repeating an earlier failure
 * (`from_review`) is not a new one, and several quotes of one session that day count once.
 * Oldest first.
 */
export function distinctObservations(evidence: ReviewEvidence[]): ReviewEvidence[] {
  const seen = new Set<string>();
  const kept: ReviewEvidence[] = [];
  for (const item of [...evidence].sort((a, b) => Date.parse(a.at) - Date.parse(b.at))) {
    if (item.from_review) continue;
    const key = `${item.workspace_id}\u0000${item.chat_id}\u0000${localDay(item.at)}`;
    if (seen.has(key)) continue;
    seen.add(key);
    kept.push(item);
  }
  return kept;
}

export interface PatternStats {
  observations: number;
  days: number;
  branches: number;
  workspaces: number;
  firstSeen: string | null;
  lastSeen: string | null;
  /** Report mentions that repeat an observation, shown apart and never counted. */
  repeats: number;
}

export function patternStats(pattern: ReviewPattern): PatternStats {
  const kept = distinctObservations(pattern.observations);
  const branchKeys = kept.filter(e=>e.branch.trim()).map((e) => `${e.workspace_id}\u0000${e.branch}`);
  return {
    observations: kept.length,
    days: new Set(kept.map((e) => localDay(e.at))).size,
    branches: new Set(branchKeys).size,
    workspaces: new Set(kept.map((e) => e.workspace_id)).size,
    firstSeen: kept[0]?.at ?? null,
    lastSeen: kept.at(-1)?.at ?? null,
    repeats: pattern.observations.filter((e) => e.from_review).length,
  };
}

export function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

/**
 * What a seven-day period says about a pattern:
 * - `exact`: every original session of the period was read and a report covers it, so the count (zero included) is the count;
 * - `at_least`: observations exist but some sessions were only sampled or no report covers it, so the count is a lower bound;
 * - `incomplete`: nothing observed, but the evidence read was partial: unknown, not zero;
 * - `none`: no report and no session read for the period.
 */
export type EvolutionCertainty = "exact" | "at_least" | "incomplete" | "none";

export interface EvolutionWeek {
  start: string;
  end: string;
  /** The distinct observations found, or null when the period's count is unknown. */
  observations: number | null;
  certainty: EvolutionCertainty;
}

function addDays(date: Date, days: number): Date {
  return new Date(date.getFullYear(), date.getMonth(), date.getDate() + days);
}

/**
 * Four seven-day periods ending at `end`, with the pattern's distinct observations in each.
 * Zero is shown only when every day of the period was read in full and a usable report covers
 * it; a report alone proves nothing about sessions it did not read.
 */
export function evolution(
  pattern: ReviewPattern,
  reports: ReviewReport[],
  coverage: ReviewSourceCoverage[],
  end: string,
): EvolutionWeek[] {
  if (!end) return [];
  const last = dayDate(end);
  const first = addDays(last, -27);
  const kept = distinctObservations(pattern.observations);
  const usable = reports.filter((r) => r.status === "complete" || r.status === "incomplete").map((r) => localDay(r.date));
  const byDay = new Map(coverage.map((c) => [localDay(c.date), c]));
  return [0, 1, 2, 3].map((week) => {
    const days = Array.from({ length: 7 }, (_, i) => dayKey(addDays(first, 7 * week + i)));
    const start = days[0] ?? "";
    const stop = days[6] ?? "";
    const reported = usable.some((day) => day >= start && day <= stop);
    const fullyRead = days.every((day) => byDay.get(day)?.complete === true);
    const anyRead = days.some((day) => (byDay.get(day)?.included ?? 0) > 0);
    const found = kept.filter((e) => {
      const day = localDay(e.at);
      return day >= start && day <= stop;
    }).length;
    if (reported && fullyRead) return { start, end: stop, observations: found, certainty: "exact" };
    if (found > 0) return { start, end: stop, observations: found, certainty: "at_least" };
    return { start, end: stop, observations: null, certainty: reported || anyRead ? "incomplete" : "none" };
  });
}

/** How a period reads in the evolution strip. */
export function evolutionText(week: EvolutionWeek): string {
  if (week.certainty === "exact") return week.observations === 0 ? "0 observed" : plural(week.observations ?? 0, "observation");
  if (week.certainty === "at_least") return `≥ ${plural(week.observations ?? 0, "observation")}`;
  return week.certainty === "incomplete" ? "Incomplete evidence" : "No evidence";
}

export interface CoverageSummary {
  /** Original source events available in the period. */
  available: number;
  /** Of those, the ones the analysis actually read. */
  included: number;
  /** Days with events where only a sample was read, oldest first. */
  sampledDays: string[];
  unknownDays: number;
  /** Whether every day of the coverage was read in full. */
  complete: boolean;
}

export function coverageSummary(coverage: ReviewSourceCoverage[]): CoverageSummary {
  const sorted = [...coverage].sort((a, b) => localDay(a.date).localeCompare(localDay(b.date)));
  return {
    available: sorted.reduce((sum, c) => sum + c.available, 0),
    included: sorted.reduce((sum, c) => sum + c.included, 0),
    sampledDays: sorted.filter((c) => c.available > 0 && !c.complete).map((c) => localDay(c.date)),
    unknownDays: Math.max(0,28-new Set(sorted.filter(c=>c.available>0).map(c=>localDay(c.date))).size),
    complete: sorted.length === 28 && sorted.every((c) => c.complete),
  };
}

function dayKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

/** Reports newest first; a weekly one before a daily one of the same day. */
export function reportsNewestFirst(reports: ReviewReport[]): ReviewReport[] {
  return [...reports].sort(
    (a, b) => localDay(b.date).localeCompare(localDay(a.date)) || (a.kind === b.kind ? 0 : a.kind === "weekly" ? -1 : 1),
  );
}

export function latestReport(reports: ReviewReport[], kind: ReviewReport["kind"]): ReviewReport | null {
  return reportsNewestFirst(reports).find((r) => r.kind === kind) ?? null;
}

/**
 * Seven-day periods of the coverage that hold no usable report, as ranges; the
 * absence can mean inactivity, a deletion or a review that never ran.
 */
export function weeksWithoutReports(reports: ReviewReport[], start: string, end: string): string[] {
  if (!start || !end) return [];
  const usable = reports.filter((r) => r.status === "complete" || r.status === "incomplete").map((r) => localDay(r.date));
  const first = dayDate(start);
  const last = dayKey(dayDate(end));
  const missing: string[] = [];
  for (let period = first; dayKey(period) <= last; period = addDays(period, 7)) {
    const from = dayKey(period);
    const to = dayKey(addDays(period, 6)) < last ? dayKey(addDays(period, 6)) : last;
    if (!usable.some((day) => day >= from && day <= to)) missing.push(dayRange(from, to));
  }
  return missing;
}

/**
 * The source revision an automatic `reviews.refresh` would analyse, or null when it should not
 * run: disabled, no report, insights current, already running, or the last run failed (a manual
 * Refresh retries). The revision moves only with completed report turns, so a report still being
 * written, or a change of its live status, does not start another model run. The page runs it
 * once per key.
 */
export function autoRefreshKey(info: ReviewsInfo): string | null {
  if (!info.auto_refresh_enabled || !info.reports.length || !info.stale || info.running || info.last_error) return null;
  return `${info.source_seq}:${info.coverage_end}`;
}

/**
 * Orders the daemon's answers: each is a whole snapshot, so one asked before a later answer was
 * applied (a poll sent before a save) must not overwrite it.
 */
export class SnapshotGate {
  private issued = 0;
  private applied = 0;
  private floor = 0;
  mutating = false;

  issue(): number {
    return ++this.issued;
  }

  /** Whether no request was sent after `ticket`: its failure is then the page's current state. */
  isLatest(ticket: number): boolean {
    return ticket === this.issued;
  }

  /** No poll may cross a user mutation, including one sent before the save reached the daemon. */
  beginMutation(): number {
    this.mutating = true;
    this.floor = this.issue();
    return this.floor;
  }

  endMutation(): void { this.mutating = false; }

  /** Whether the answer to request `ticket` may replace the data; it then becomes the latest. */
  accept(ticket: number): boolean {
    if (ticket < this.floor || ticket <= this.applied) return false;
    this.applied = ticket;
    return true;
  }
}

/** What "Update type" offers; `keep` records a note only (a deferral, a refusal, context): no status. */
export type FollowUpChoice = "keep" | ReviewRecommendationStatus;

export interface FollowUpDraft {
  choice: FollowUpChoice;
  note: string;
  sourceSeq: number | null;
}

export const EMPTY_DRAFT: FollowUpDraft = { choice: "keep", note: "", sourceSeq: null };

/**
 * Why the draft cannot be saved, in one sentence, or null. A note only needs its text, whatever
 * the status shown; an explicit observed effect also needs its source and measurement.
 */
export function followUpProblem(draft: FollowUpDraft): string | null {
  const note = draft.note.trim();
  if (note.length < 10 || note.length > 6000) return "Add a note describing the decision or result in 10–6000 characters.";
  if (draft.choice !== "observed_effect") return null;
  if (draft.sourceSeq == null) return "An observed effect needs the conversation it was observed in: pick a source.";
  if (note.length < 20 || !/\d/.test(note)) {
    return "An observed effect needs its measurement and date in the note (e.g. “1 CI rework over 3 reviews, Nov 9–12”).";
  }
  return null;
}

/** The status a draft records: the one chosen, or null for a note that changes no status. */
export function draftStatus(draft: FollowUpDraft): ReviewRecommendationStatus | null {
  return draft.choice === "keep" ? null : draft.choice;
}

export interface Submission {
  key: string;
  draft: FollowUpDraft;
  /** Frozen at the first attempt, so a retry sends what the user meant then. */
  status: ReviewRecommendationStatus | null;
}

interface DraftEntry {
  draft: FollowUpDraft;
  submission: Submission | null;
  failure: string | null;
}

/**
 * The follow-up drafts of the page, one per experiment: switching experiments, a poll that
 * reorders the list or leaving the tab keeps what the user typed, and a retry after a failure
 * reuses the same idempotency key and status.
 */
export class FollowUpDrafts {
  private entries = new Map<string, DraftEntry>();

  private entry(experimentId: string): DraftEntry {
    return this.entries.get(experimentId) ?? { draft: EMPTY_DRAFT, submission: null, failure: null };
  }

  draft(experimentId: string): FollowUpDraft {
    return this.entry(experimentId).draft;
  }

  failure(experimentId: string): string | null {
    return this.entry(experimentId).failure;
  }

  edit(experimentId: string, draft: FollowUpDraft): void {
    this.entries.set(experimentId, { ...this.entry(experimentId), draft });
  }

  /** The submission for the current draft: the previous one when the draft is unchanged (a retry), else a new one. */
  submit(experimentId: string, newKey: () => string): Submission {
    const entry = this.entry(experimentId);
    const previous = entry.submission;
    const submission: Submission =
      previous && sameDraft(previous.draft, entry.draft)
        ? previous
        : { key: newKey(), draft: entry.draft, status: draftStatus(entry.draft) };
    this.entries.set(experimentId, { ...entry, submission, failure: null });
    return submission;
  }

  failed(experimentId: string, failure: string): void {
    this.entries.set(experimentId, { ...this.entry(experimentId), failure });
  }

  /** Forgets a saved draft, unless the user changed it while it was being saved. */
  saved(experimentId: string, submission: Submission): void {
    const entry = this.entry(experimentId);
    if (sameDraft(entry.draft, submission.draft)) this.entries.delete(experimentId);
    else this.entries.set(experimentId, { ...entry, submission: null, failure: null });
  }
}

function sameDraft(a: FollowUpDraft, b: FollowUpDraft): boolean {
  return a.choice === b.choice && a.note === b.note && a.sourceSeq === b.sourceSeq;
}

/**
 * The experiment the detail shows: the one pinned by the page while it still exists, else the
 * first of the list. The page pins it, so newer data that reorders the list never moves it.
 */
export function shownExperiment(experiments: ReviewExperiment[], pinned: string | null): ReviewExperiment | null {
  return experiments.find((e) => e.id === pinned) ?? experiments[0] ?? null;
}

/** The conversations an update may cite: the experiment's evidence, then its pattern's original observations. */
export function sourceChoices(experiment: ReviewExperiment, pattern: ReviewPattern | null): ReviewEvidence[] {
  const seen = new Set<number>();
  return [...experiment.evidence, ...(pattern ? distinctObservations(pattern.observations) : [])].filter((e) => {
    if (seen.has(e.source_seq)) return false;
    seen.add(e.source_seq);
    return true;
  });
}

/** The workspaces the evidence comes from, by id, with the name shown; for the scope filter. */
export function evidenceWorkspaces(info: ReviewsInfo): Array<{ id: string; name: string }> {
  const names = new Map<string, string>();
  const projection = info.projection;
  if (!projection) return [];
  for (const pattern of projection.patterns) {
    for (const e of [...pattern.observations, ...pattern.counterexamples]) names.set(e.workspace_id, e.workspace);
  }
  for (const experiment of projection.experiments) for (const e of experiment.evidence) names.set(e.workspace_id, e.workspace);
  return [...names].map(([id, name]) => ({ id, name: name || id })).sort((a, b) => a.name.localeCompare(b.name));
}

export function patternInScope(pattern: ReviewPattern, workspaceId: string | null): boolean {
  return !workspaceId || pattern.observations.some((e) => e.workspace_id === workspaceId);
}

export function experimentInScope(experiment: ReviewExperiment, pattern: ReviewPattern | null, workspaceId: string | null): boolean {
  return (
    !workspaceId ||
    experiment.evidence.some((e) => e.workspace_id === workspaceId) ||
    (pattern !== null && patternInScope(pattern, workspaceId))
  );
}

/** Experiments still waiting on the user first (proposed, decided, tried, unknown), newest origin first. */
export function experimentsByAttention(experiments: ReviewExperiment[], followups: ReviewFollowUp[]): ReviewExperiment[] {
  const rank = (e: ReviewExperiment) => (effectiveStatus(e, followups) === "observed_effect" ? 1 : 0);
  return [...experiments].sort((a, b) => rank(a) - rank(b) || localDay(b.origin_date).localeCompare(localDay(a.origin_date)));
}
