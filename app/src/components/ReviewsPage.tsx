// The Reviews page: what repeats across the daily and weekly review reports, the experiments
// they propose, the user's dated follow-ups, and the reports themselves. The source
// conversations stay the evidence: every claim links back to one through `onOpenSource`.
import type { ReviewEvidence, ReviewExperiment, ReviewPattern, ReviewReport, ReviewsInfo } from "@chauffeur/protocol";
import { ChevronLeft, ChevronRight, ClipboardList, ExternalLink, RefreshCw, Settings } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { errorMessage } from "@/lib/rpc";
import {
  type CoverageSummary,
  type FollowUpChoice,
  type FollowUpDraft,
  FollowUpDrafts,
  SnapshotGate,
  autoRefreshKey,
  coverageSummary,
  dayRange,
  distinctObservations,
  effectiveStatus,
  evidenceWorkspaces,
  evolution,
  evolutionText,
  experimentInScope,
  experimentsByAttention,
  followUpProblem,
  followUpsOf,
  latestReport,
  localTime,
  patternInScope,
  patternStats,
  plural,
  reportStatusLabel,
  reportStatusTone,
  reportsNewestFirst,
  shortDay,
  shownExperiment,
  sourceChoices,
  statusLabel,
  statusTone,
  trendLabel,
  trendTone,
  weeksWithoutReports,
} from "@/lib/reviews";
import { client, openSettings, toast, useApp } from "@/state/app";
import { ShowSidebarButton } from "./ShowSidebarButton";
import { Badge, Button, EmptyState, IconButton, Select, type SelectOption, Spinner, cx, inputClass } from "./ui";

type OpenSource = (workspaceId: string, chatId: string, sourceSeq?: number) => Promise<void>;
type Tab = "overview" | "experiments" | "history";

/** How often the page asks for news while an analysis runs. */
const POLL_MS = 3000;

// Opening an evidence conversation must not discard the unsaved notes in this window.
const sessionDrafts = new FollowUpDrafts();

export function ReviewsPage({ onOpenSource }: { onOpenSource: OpenSource }) {
  const [info, setInfo] = useState<ReviewsInfo | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [requesting, setRequesting] = useState(false);
  const [tab, setTab] = useState<Tab>("overview");
  const [patternId, setPatternId] = useState<string | null>(null);
  const [experimentId, setExperimentId] = useState<string | null>(null);
  const [scope, setScope] = useState<string | null>(null);
  const gate = useRef(new SnapshotGate());
  const attempted = useRef(new Set<string>());
  const drafts = useRef(sessionDrafts);

  const accept = useCallback((ticket: number, next: ReviewsInfo) => {
    if (!gate.current.accept(ticket)) return;
    setInfo(next);
    setLoadError(null);
  }, []);

  const load = useCallback(async () => {
    if (gate.current.mutating) return;
    const ticket = gate.current.issue();
    try {
      accept(ticket, await client.call("reviews.info", {}));
    } catch (error) {
      if (gate.current.isLatest(ticket)) setLoadError(errorMessage(error));
    }
  }, [accept]);

  const refresh = useCallback(async () => {
    if (gate.current.mutating) return;
    const ticket = gate.current.issue();
    setRequesting(true);
    try {
      accept(ticket, await client.call("reviews.refresh", {}));
    } catch (error) {
      toast(`Could not refresh the review insights: ${errorMessage(error)}`, "error");
    } finally {
      setRequesting(false);
    }
  }, [accept]);

  useEffect(() => void load(), [load]);

  // Builds the insights once per set of new reports; a failed run waits for a manual Refresh.
  useEffect(() => {
    const key = info && autoRefreshKey(info);
    if (!key || attempted.current.has(key)) return;
    attempted.current.add(key);
    void refresh();
  }, [info, refresh]);

  const running = info?.running ?? false;
  useEffect(() => {
    const timer = window.setInterval(() => void load(), running ? POLL_MS : 15000);
    return () => window.clearInterval(timer);
  }, [running, load]);

  // A review workspace that appears, ends or is archived may bring a report: read the info again.
  const reviewSignature = useApp((s) =>
    Object.values(s.workspaces)
      .filter((w) => w.daily_review_date)
      .map((w) => `${w.id}:${w.state}:${w.activity}`)
      .sort()
      .join("|"),
  );
  const firstSignature = useRef(reviewSignature);
  useEffect(() => {
    if (reviewSignature === firstSignature.current) return;
    firstSignature.current = reviewSignature;
    const timer = window.setTimeout(() => void load(), 1500);
    return () => window.clearTimeout(timer);
  }, [reviewSignature, load]);

  const open = useCallback(
    async (workspaceId: string, chatId: string, sourceSeq?: number) => {
      try {
        await onOpenSource(workspaceId, chatId, sourceSeq);
      } catch (error) {
        toast(`Could not open the source conversation: ${errorMessage(error)}`, "error");
      }
    },
    [onOpenSource],
  );

  const showTab = (next: Tab) => {
    setTab(next);
    setPatternId(null);
  };
  const pattern = info?.projection?.patterns.find((p) => p.id === patternId) ?? null;

  return (
    <section className="flex min-h-0 flex-1 flex-col" data-testid="reviews-page">
      <header className="flex h-11 shrink-0 items-center gap-2 border-b border-line px-4" data-tauri-drag-region>
        <ShowSidebarButton className="-ml-1.5" />
        <ClipboardList size={14} className="shrink-0 text-muted" />
        <h1 className="min-w-0 truncate text-[13px] font-medium" data-tauri-drag-region>
          Reviews{pattern && <span className="text-muted"> / Pattern</span>}
        </h1>
        <div className="ml-auto flex shrink-0 items-center gap-1.5">
          {running && (
            <span className="flex items-center gap-1.5 text-[12px] text-muted" data-testid="reviews-running">
              <Spinner size={11} /> <span className="hidden sm:inline">Analyzing reports…</span>
            </span>
          )}
          <Button
            size="sm"
            variant="ghost"
            onClick={() => void refresh()}
            disabled={running || requesting || !info?.reports.length}
            title={info?.reports.length ? "Analyze the review reports again with the daily review model" : "No report to analyze yet"}
            data-testid="reviews-refresh"
          >
            <RefreshCw size={12} /> <span className="hidden sm:inline">Refresh insights</span>
          </Button>
          <IconButton title="Daily review settings" onClick={() => openSettings(true, "daily-review")}>
            <Settings size={14} />
          </IconButton>
        </div>
      </header>
      <nav className="flex h-9 shrink-0 items-stretch gap-5 border-b border-line px-4" aria-label="Reviews sections">
        {(
          [
            ["overview", "Overview"],
            ["experiments", "Experiments"],
            ["history", "Report history"],
          ] as const
        ).map(([value, label]) => (
          <button
            key={value}
            type="button"
            onClick={() => showTab(value)}
            aria-current={tab === value ? "page" : undefined}
            data-testid={`reviews-tab-${value}`}
            className={cx(
              "-mb-px border-b-2 text-[12.5px] transition-colors",
              tab === value ? "border-accent text-fg" : "border-transparent text-muted hover:text-fg",
            )}
          >
            {label}
          </button>
        ))}
      </nav>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[1000px] px-4 py-6 sm:px-8">
          {!info ? (
            loadError ? (
              <div role="alert" className="rounded-lg border border-line p-4 text-[12.5px]" data-testid="reviews-load-error">
                Could not load the reviews: {loadError}{" "}
                <Button size="sm" onClick={() => void load()}>Retry</Button>
              </div>
            ) : (
              <EmptyState icon={<Spinner size={16} />} title="Loading reviews…" />
            )
          ) : (
            <>
              <Notices info={info} loadError={loadError} requesting={requesting} onRetryLoad={load} onRefresh={refresh} />
              {tab === "overview" && pattern ? (
                <PatternView
                  info={info}
                  pattern={pattern}
                  open={open}
                  onBack={() => setPatternId(null)}
                  onExperiment={(id) => {
                    setExperimentId(id);
                    showTab("experiments");
                  }}
                />
              ) : tab === "overview" ? (
                <Overview
                  info={info}
                  scope={scope}
                  onScope={setScope}
                  open={open}
                  onPattern={setPatternId}
                  onExperiment={(id) => {
                    setExperimentId(id);
                    showTab("experiments");
                  }}
                  onTab={showTab}
                  onRefresh={refresh}
                  requesting={requesting}
                />
              ) : tab === "experiments" ? (
                <Experiments
                  info={info}
                  selected={experimentId}
                  onSelect={setExperimentId}
                  open={open}
                  gate={gate.current}
                  drafts={drafts.current}
                  onSaved={accept}
                />
              ) : (
                <History info={info} open={open} />
              )}
            </>
          )}
        </div>
      </div>
    </section>
  );
}

/** What the data shown may be missing: a reload that failed, an analysis running or failed, newer reports. */
function Notices({
  info,
  loadError,
  requesting,
  onRetryLoad,
  onRefresh,
}: {
  info: ReviewsInfo;
  loadError: string | null;
  requesting: boolean;
  onRetryLoad: () => Promise<void>;
  onRefresh: () => Promise<void>;
}) {
  const notices: ReactNode[] = [];
  if (loadError) {
    notices.push(
      <Notice key="load" tone="danger" testId="reviews-reload-error">
        Could not reload the reviews: {loadError}. This page shows what was loaded before.
        <Button size="xs" className="ml-2" onClick={() => void onRetryLoad()}>Retry</Button>
      </Notice>,
    );
  }
  if (info.running) {
    notices.push(
      <Notice key="running" testId="reviews-analyzing">
        Analyzing {plural(info.reports.length, "report")} with the daily review model. The insights below stay until it finishes.
      </Notice>,
    );
  } else if (info.last_error) {
    notices.push(
      <Notice key="error" tone="danger" testId="reviews-analysis-error">
        The last analysis failed: {info.last_error}. The reports and their conversations remain available
        {info.projection ? ", and so do the earlier insights" : ""}.
        <Button size="xs" className="ml-2" busy={requesting} onClick={() => void onRefresh()}>Try again</Button>
      </Notice>,
    );
  } else if (info.stale && info.projection) {
    notices.push(
      <Notice key="stale" testId="reviews-stale">
        These insights need refreshing; they were built {shortDay(info.projection.generated_at)}.
        <Button size="xs" className="ml-2" busy={requesting} onClick={() => void onRefresh()}>Refresh insights</Button>
      </Notice>,
    );
  }
  if (!notices.length) return null;
  return <div className="mb-5 flex flex-col gap-2">{notices}</div>;
}

function Notice({ tone = "neutral", testId, children }: { tone?: "neutral" | "danger"; testId: string; children: ReactNode }) {
  return (
    <div
      role={tone === "danger" ? "alert" : "status"}
      data-testid={testId}
      className={cx(
        "rounded-md border px-3 py-2 text-[12px] leading-relaxed",
        tone === "danger" ? "border-danger/30 bg-danger/10 text-fg" : "border-line bg-panel text-muted",
      )}
    >
      {children}
    </div>
  );
}

function SourceLink({
  evidence,
  label,
  open,
}: {
  evidence: Pick<ReviewEvidence, "workspace_id" | "chat_id" | "source_seq">;
  label: string;
  open: OpenSource;
}) {
  return (
    <button
      type="button"
      className="inline-flex items-center gap-1 text-left text-accent hover:underline"
      onClick={() => void open(evidence.workspace_id, evidence.chat_id, evidence.source_seq)}
      data-testid="reviews-source-link"
    >
      {label}
      <ExternalLink size={11} className="shrink-0" />
    </button>
  );
}

function ReportLink({ report, label, open }: { report: ReviewReport; label?: string; open: OpenSource }) {
  const text = label ?? `Open ${report.kind} · ${shortDay(report.date)}`;
  if (!report.workspace_id || !report.chat_id) {
    return <span className="text-faint" title="This report's conversation is no longer available">Conversation unavailable</span>;
  }
  const { workspace_id, chat_id } = report;
  return (
    <button
      type="button"
      className="inline-flex items-center gap-1 text-left text-accent hover:underline"
      onClick={() => void open(workspace_id, chat_id)}
      data-testid="reviews-report-link"
    >
      {text}
      <ExternalLink size={11} className="shrink-0" />
    </button>
  );
}

function PageTitle({ title, subtitle, children }: { title: string; subtitle: string; children?: ReactNode }) {
  return (
    <div className="mb-5 flex flex-wrap items-start justify-between gap-3">
      <div className="min-w-0">
        <h2 className="text-[17px] font-semibold">{title}</h2>
        <p className="mt-1 text-[12.5px] text-muted">{subtitle}</p>
      </div>
      {children}
    </div>
  );
}

function SectionHead({ title, aside }: { title: string; aside?: ReactNode }) {
  return (
    <div className="mb-2 mt-7 flex flex-wrap items-baseline justify-between gap-2">
      <h3 className="text-[13px] font-semibold">{title}</h3>
      {aside && <span className="text-[12px] text-muted">{aside}</span>}
    </div>
  );
}

function nextSynthesis(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  return `${date.toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric" })} at ${localTime(iso)}`;
}

function Overview({
  info,
  scope,
  onScope,
  open,
  onPattern,
  onExperiment,
  onTab,
  onRefresh,
  requesting,
}: {
  info: ReviewsInfo;
  scope: string | null;
  onScope: (scope: string | null) => void;
  open: OpenSource;
  onPattern: (id: string) => void;
  onExperiment: (id: string) => void;
  onTab: (tab: Tab) => void;
  onRefresh: () => Promise<void>;
  requesting: boolean;
}) {
  const projection = info.projection;
  const workspaces = useMemo(() => evidenceWorkspaces(info), [info]);
  const usedReports = info.reports.filter(r => projection?.report_ids.includes(r.id));
  const weekly = latestReport(usedReports, "weekly");
  const daily = latestReport(usedReports, "daily");
  const scopeOptions: Array<SelectOption<string>> = [
    { value: "", label: "All workspaces" },
    ...workspaces.map((w) => ({ value: w.id, label: w.name })),
  ];
  const title = (
    <PageTitle title="Reviews" subtitle="What repeats, what to try, and what actually changed.">
      {workspaces.length > 1 && (
        <Select
          value={scope ?? ""}
          options={scopeOptions}
          onChange={(value) => onScope(value || null)}
          title="Scope of the patterns and experiments below"
          testId="reviews-scope"
          className="border border-line"
        />
      )}
    </PageTitle>
  );

  if (!projection) {
    return (
      <div data-testid="reviews-overview">
        {title}
        {info.running ? (
          <EmptyState icon={<Spinner size={16} />} title={`Building insights from ${plural(info.reports.length, "report")}…`}>
            The report history is already available.
          </EmptyState>
        ) : !info.reports.length ? (
          <EmptyState title="No review reports yet">
            Daily reviews run at 18:00 on days with activity, and a weekly synthesis on Sundays. Insights appear here once there are reports.
            <div className="mt-3">
              <Button size="sm" onClick={() => openSettings(true, "daily-review")}>
                <Settings size={12} /> Daily review settings
              </Button>
            </div>
          </EmptyState>
        ) : (
          <EmptyState title="No insights yet">
            {plural(info.reports.length, "report")} can be analyzed with the daily review model.
            <div className="mt-3 flex justify-center gap-2">
              <Button size="sm" busy={requesting} onClick={() => void onRefresh()}>
                <RefreshCw size={12} /> Build insights
              </Button>
              <Button size="sm" variant="ghost" onClick={() => onTab("history")}>Report history</Button>
            </div>
          </EmptyState>
        )}
      </div>
    );
  }

  const [headline, ...rest] = projection.overview.trim().split(/\n+/);
  const patternsById = new Map(projection.patterns.map((p) => [p.id, p]));
  const patterns = projection.patterns.filter((p) => patternInScope(p, scope));
  const experiments = experimentsByAttention(
    projection.experiments.filter((e) => experimentInScope(e, (e.pattern_id && patternsById.get(e.pattern_id)) || null, scope)),
    info.followups,
  );
  const incomplete = info.reports.filter((r) => r.status !== "complete");
  const archived = info.reports.some((r) => r.archived);

  return (
    <div data-testid="reviews-overview">
      {title}
      <div className="rounded-lg border border-line bg-panel px-5 py-4" data-testid="reviews-direction">
        <div className="text-[11px] font-medium uppercase tracking-wide text-muted">
          {`Review synthesis · Built ${shortDay(projection.generated_at)}`}{scope && " · Across all workspaces"}
          {daily && ` · Follow-up through ${shortDay(daily.date)}`}
        </div>
        {headline && <p className="mt-2 text-[14.5px] font-medium leading-snug">{headline}</p>}
        {rest.length > 0 && <p className="mt-2 whitespace-pre-line text-[12.5px] leading-relaxed text-muted">{rest.join("\n")}</p>}
        <div className="mt-3 flex flex-wrap items-center justify-between gap-2 text-[12.5px]">
          {weekly ? <ReportLink report={weekly} label={`Open weekly review · ${shortDay(weekly.date)}`} open={open} /> : <span />}
          {info.next_weekly && <span className="text-muted">Next synthesis · {nextSynthesis(info.next_weekly)}</span>}
        </div>
      </div>

      <SectionHead
        title="Recurring patterns"
        aside={
          coverageSummary(projection.source_coverage).complete
            ? "Distinct observations, not report mentions"
            : "Distinct observations in the sessions read, not report mentions"
        }
      />
      {patterns.length ? (
        <div className="divide-y divide-line overflow-hidden rounded-lg border border-line">
          {patterns.map((p) => {
            const stats = patternStats(p);
            return (
              <button
                key={p.id}
                type="button"
                onClick={() => onPattern(p.id)}
                data-testid="reviews-pattern-row"
                className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-hover"
              >
                <div className="min-w-0 flex-1">
                  <div className="text-[13px] font-medium">{p.title}</div>
                  <div className="mt-1 text-[12px] text-muted">
                    {plural(stats.days, "separate day")}{stats.branches > 0 && ` · ${plural(stats.branches, "branch", "branches")}`} · {plural(stats.workspaces, "workspace")}
                  </div>
                </div>
                <div className="flex shrink-0 flex-col items-end gap-1">
                  <Badge tone={trendTone(p.trend)}>{trendLabel(p.trend)}</Badge>
                  <span className="text-[12px] text-muted">{plural(stats.observations, "distinct observation")}</span>
                </div>
                <ChevronRight size={14} className="shrink-0 text-faint" />
              </button>
            );
          })}
        </div>
      ) : (
        <p className="text-[12.5px] text-muted">No recurring pattern in the covered reports{scope ? " for this workspace" : ""}.</p>
      )}

      <SectionHead
        title="Experiments to follow"
        aside={experiments.length > 3 ? <button type="button" className="text-accent hover:underline" onClick={() => onTab("experiments")}>All {experiments.length} experiments</button> : undefined}
      />
      {experiments.length ? (
        <div className="divide-y divide-line overflow-hidden rounded-lg border border-line">
          {experiments.slice(0, 3).map((e) => {
            const status = effectiveStatus(e, info.followups);
            const updates = followUpsOf(e.id, info.followups);
            const last = updates.at(-1);
            const confirmed = updates.findLast(f => f.status !== null);
            return (
              <button
                key={e.id}
                type="button"
                onClick={() => onExperiment(e.id)}
                data-testid="reviews-experiment-row"
                className="flex w-full flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3 text-left hover:bg-hover"
              >
                <div className="min-w-0 flex-1 basis-64">
                  <div className="text-[13px] font-medium">{e.title}</div>
                  <div className="mt-1 line-clamp-2 text-[12px] text-muted">{last ? `${last.status === null ? "Note: " : ""}${last.note}` : e.success_measure}</div>
                </div>
                <div className="flex w-44 shrink-0 flex-col items-start gap-1">
                  <Badge tone={statusTone(status)}>{statusLabel(status)}</Badge>
                  <span className="text-[12px] text-muted">
                    {confirmed ? `Recorded ${shortDay(confirmed.recorded_at)}` : e.next_check ? `Check again ${shortDay(e.next_check)}` : status === "proposed" ? "No decision recorded" : "From the reports"}
                  </span>
                </div>
                <span className="flex w-24 shrink-0 items-center gap-1 text-[12.5px] text-accent">
                  {status === "proposed" ? "Review" : status === "observed_effect" ? "Results" : "Follow up"} <ChevronRight size={12} />
                </span>
              </button>
            );
          })}
        </div>
      ) : (
        <p className="text-[12.5px] text-muted">No experiment proposed in the covered reports{scope ? " for this workspace" : ""}.</p>
      )}

      {projection.gaps.length > 0 && (
        <ul className="mt-4 list-disc space-y-1 pl-5 text-[12px] text-muted" data-testid="reviews-gaps">
          {projection.gaps.map((gap) => <li key={gap}>{gap}</li>)}
        </ul>
      )}
      <div className="mt-4 flex flex-wrap items-center justify-between gap-2 text-[12px] text-muted">
        <span>
          {info.coverage_start && info.coverage_end ? `Reports ${dayRange(info.coverage_start, info.coverage_end)}` : plural(info.reports.length, "report")}
          {incomplete.length > 0 && ` · ${plural(incomplete.length, "report")} not complete`}
          {archived && " · Archived conversations included"}
        </span>
        <button type="button" className="flex items-center gap-1 text-accent hover:underline" onClick={() => onTab("history")}>
          Sources & coverage <ChevronRight size={12} />
        </button>
      </div>
    </div>
  );
}

function PatternView({
  info,
  pattern,
  open,
  onBack,
  onExperiment,
}: {
  info: ReviewsInfo;
  pattern: ReviewPattern;
  open: OpenSource;
  onBack: () => void;
  onExperiment: (id: string) => void;
}) {
  const stats = patternStats(pattern);
  const observations = distinctObservations(pattern.observations);
  const sources = info.projection?.source_coverage ?? [];
  const coverage = coverageSummary(sources);
  const weeks = evolution(pattern, info.reports, sources, info.coverage_end || (info.projection?.generated_at ?? ""));
  const related = info.projection?.experiments.filter((e) => e.pattern_id === pattern.id) ?? [];
  const reports = info.reports.filter((r) => pattern.report_ids.includes(r.id));

  return (
    <div data-testid="reviews-pattern">
      <nav className="mb-2 text-[12px]">
        <button type="button" className="inline-flex items-center gap-0.5 text-accent hover:underline" onClick={onBack} data-testid="reviews-pattern-back">
          <ChevronLeft size={12} /> Reviews
        </button>
        <span className="text-muted"> / Pattern</span>
      </nav>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <h2 className="min-w-0 text-[17px] font-semibold">{pattern.title}</h2>
        <Badge tone={trendTone(pattern.trend)}>{trendLabel(pattern.trend)}</Badge>
      </div>
      <p className="mt-1 text-[12.5px] text-muted">
        {stats.firstSeen && `First seen ${shortDay(stats.firstSeen)}`}
        {stats.lastSeen && ` · Last seen ${shortDay(stats.lastSeen)}`}
        {info.projection && ` · Insights built ${shortDay(info.projection.generated_at)}`}
      </p>
      {pattern.summary && <p className="mt-2 text-[12.5px] leading-relaxed">{pattern.summary}</p>}
      <div className="mt-3 flex flex-wrap gap-1.5">
        {[
          `${plural(stats.observations, "distinct observation")}${coverage.complete ? "" : " found"}`,
          plural(stats.days, "separate day"),
          ...(stats.branches ? [plural(stats.branches, "branch", "branches")] : []),
          plural(stats.workspaces, "workspace"),
        ].map((chip) => <Badge key={chip}>{chip}</Badge>)}
      </div>

      <div className="mt-2 grid gap-x-7 lg:grid-cols-[minmax(0,1fr)_280px]">
        <div className="min-w-0">
          <SectionHead title="Original observations" />
          <p className="-mt-1 mb-2 text-[12px] text-muted">One row per session and day; a report that repeats an earlier failure is not a new row.</p>
          <SourceCoverageNote coverage={coverage} />
          {observations.length ? (
            <div className="overflow-x-auto">
              <table className="w-full min-w-[520px] text-[12.5px]" data-testid="reviews-observations">
                <thead>
                  <tr className="border-b border-line text-left text-[12px] text-muted">
                    <th className="py-2 pr-3 font-medium">Day</th>
                    <th className="py-2 pr-3 font-medium">Workspace / branch</th>
                    <th className="py-2 font-medium">Evidence</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-line">
                  {observations.map((e) => (
                    <tr key={e.id} className="align-top">
                      <td className="py-2.5 pr-3 whitespace-nowrap">
                        <div>{shortDay(e.at)}</div>
                        {localTime(e.at) && <div className="text-[11.5px] text-muted">{localTime(e.at)} local</div>}
                      </td>
                      <td className="py-2.5 pr-3">
                        <div>{e.workspace}</div>
                        {e.branch && <div className="text-[11.5px] text-muted">{e.branch}</div>}
                      </td>
                      <td className="py-2.5">
                        <SourceLink evidence={e} label={`Open session · ${shortDay(e.at)}`} open={open} />
                        {e.human && <span className="ml-2 text-[11px] text-faint" title="A message you typed in that session">Your message</span>}
                        {e.quote && <div className="mt-0.5 line-clamp-4 max-w-[520px] break-words text-[12px] text-muted">{e.quote}</div>}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="text-[12.5px] text-muted">The reports cite no original session for this pattern.</p>
          )}
          {stats.repeats > 0 && (
            <p className="mt-2 border-l-2 border-line-strong pl-3 text-[12px] text-muted">
              {plural(stats.repeats, "report mention")} repeat{stats.repeats === 1 ? "s" : ""} an earlier observation and add no new one.
            </p>
          )}

          {weeks.length > 0 && <>
          <SectionHead title="Evolution over four weeks" />
          <div className="grid grid-cols-2 overflow-hidden rounded-lg border border-line sm:grid-cols-4" data-testid="reviews-evolution">
            {weeks.map((week) => (
              <div
                key={week.start}
                className="border-line px-3 py-2 [&:not(:last-child)]:border-r"
                data-testid="reviews-evolution-period"
                data-certainty={week.certainty}
              >
                <div className="text-[11px] uppercase tracking-wide text-muted">{dayRange(week.start, week.end)}</div>
                <div
                  className={cx("mt-1 text-[12.5px]", week.observations === null && "text-faint")}
                  title={
                    week.certainty === "at_least"
                      ? "Some sessions of this period were only sampled or no report covers it: there may be more."
                      : week.certainty === "incomplete"
                        ? "Not every session of this period was read: none found is not the same as none."
                        : undefined
                  }
                >
                  {evolutionText(week)}
                </div>
              </div>
            ))}
          </div>
          <p className="mt-2 text-[12px] text-muted">
            Zero is shown only for a period whose sessions were all read and that a report covers. “≥” marks a count from sampled sessions; a period with
            incomplete or no evidence is unknown, not a period without the problem.
          </p>
          </>}

          <SectionHead title="Counterexamples & gaps" />
          {pattern.counterexamples.length === 0 && pattern.gaps.length === 0 ? (
            <p className="text-[12.5px] text-muted">The reports name no counterexample or gap.</p>
          ) : (
            <ul className="space-y-1.5 text-[12.5px]">
              {pattern.counterexamples.map((e) => (
                <li key={e.id}>
                  {shortDay(e.at)} · {e.workspace} · {e.quote}{" "}
                  <SourceLink evidence={e} label={`Open ${shortDay(e.at)} session`} open={open} />
                </li>
              ))}
              {pattern.gaps.map((gap) => <li key={gap} className="text-muted">{gap}</li>)}
            </ul>
          )}
        </div>

        <aside className="mt-7 flex flex-col gap-3">
          <div className="rounded-lg border border-line bg-panel p-4">
            <div className="text-[11px] font-medium uppercase tracking-wide text-muted">Confidence</div>
            <p className="mt-2 text-[12.5px] leading-relaxed">{pattern.confidence || "Not stated by the analysis."}</p>
          </div>
          {related.map((e) => {
            const status = effectiveStatus(e, info.followups);
            return (
              <div key={e.id} className="rounded-lg border border-line bg-panel p-4" data-testid="reviews-related-experiment">
                <div className="text-[11px] font-medium uppercase tracking-wide text-muted">Related experiment · {shortDay(e.origin_date)}</div>
                <div className="mt-2 text-[13px] font-medium">{e.title}</div>
                <p className="mt-1 text-[12px] leading-relaxed text-muted">{e.experiment}</p>
                <div className="mt-2"><Badge tone={statusTone(status)}>{statusLabel(status)}</Badge></div>
                {(e.expected_benefit || e.effort) && (
                  <p className="mt-2 text-[12px] leading-relaxed text-muted">
                    {e.expected_benefit && `Expected benefit: ${e.expected_benefit}`}
                    {e.effort && ` Effort: ${e.effort}`}
                  </p>
                )}
                <Button size="sm" className="mt-3" onClick={() => onExperiment(e.id)}>
                  Review experiment <ChevronRight size={12} />
                </Button>
              </div>
            );
          })}
          {reports.length > 0 && (
            <div className="px-1 text-[12px] text-muted">
              <div>Summarized in</div>
              <div className="mt-1 flex flex-col items-start gap-1">
                {reports.map((r) => <ReportLink key={r.id} report={r} label={`${r.kind === "weekly" ? "Weekly" : "Daily"} review · ${shortDay(r.date)}`} open={open} />)}
              </div>
              <p className="mt-2">Original sessions remain the evidence.</p>
            </div>
          )}
        </aside>
      </div>
    </div>
  );
}

const CHOICES: Array<{ value: Exclude<FollowUpChoice, "keep">; label: string }> = [
  { value: "decided", label: "Decided — I will try this" },
  { value: "tried", label: "Tried — record what I did" },
  { value: "observed_effect", label: "Observed effect — add results" },
  { value: "effect_unknown", label: "Effect unknown — insufficient evidence" },
];

function Experiments({
  info,
  selected,
  onSelect,
  open,
  gate,
  drafts,
  onSaved,
}: {
  info: ReviewsInfo;
  selected: string | null;
  onSelect: (id: string) => void;
  open: OpenSource;
  gate: SnapshotGate;
  drafts: FollowUpDrafts;
  onSaved: (ticket: number, info: ReviewsInfo) => void;
}) {
  const experiments = experimentsByAttention(info.projection?.experiments ?? [], info.followups);
  const experiment = shownExperiment(experiments, selected);
  // Pins what is shown, so newer data that reorders the list never swaps the experiment being edited.
  const shownId = experiment?.id ?? null;
  useEffect(() => {
    if (shownId && shownId !== selected) onSelect(shownId);
  }, [shownId, selected, onSelect]);
  const title = <PageTitle title="Experiments" subtitle="A suggestion becomes a decision only when you record one." />;
  if (!experiment) {
    return (
      <div data-testid="reviews-experiments">
        {title}
        <EmptyState title={info.projection ? "No experiment proposed" : "No insights yet"}>
          {info.projection
            ? "The covered reports propose no experiment."
            : "Experiments appear once the reports are analyzed. Refresh insights from the top bar."}
        </EmptyState>
      </div>
    );
  }
  return (
    <div data-testid="reviews-experiments">
      {title}
      <div className="grid gap-6 md:grid-cols-[250px_minmax(0,1fr)]">
        <div>
          <div className="overflow-hidden rounded-lg border border-line">
            {experiments.map((e) => {
              const status = effectiveStatus(e, info.followups);
              const active = e.id === experiment.id;
              return (
                <button
                  key={e.id}
                  type="button"
                  onClick={() => onSelect(e.id)}
                  aria-current={active ? "true" : undefined}
                  data-testid="reviews-experiment-item"
                  className={cx(
                    "block w-full border-b border-l-2 border-b-line px-3.5 py-3 text-left last:border-b-0",
                    active ? "border-l-accent bg-hover" : "border-l-transparent hover:bg-hover",
                  )}
                >
                  <div className="text-[12.5px] font-medium leading-snug">{e.title}</div>
                  <div className="mt-2"><Badge tone={statusTone(status)}>{statusLabel(status)}</Badge></div>
                </button>
              );
            })}
          </div>
          <p className="mt-3 border-l-2 border-line pl-3 text-[12px] leading-relaxed text-muted">
            Statuses describe evidence, not a forced sequence: Proposed · Decided · Tried · Observed effect · Effect unknown.
          </p>
        </div>
        <ExperimentDetail key={experiment.id} info={info} experiment={experiment} open={open} gate={gate} drafts={drafts} onSaved={onSaved} />
      </div>
    </div>
  );
}

function ExperimentDetail({
  info,
  experiment,
  open,
  gate,
  drafts,
  onSaved,
}: {
  info: ReviewsInfo;
  experiment: ReviewExperiment;
  open: OpenSource;
  gate: SnapshotGate;
  drafts: FollowUpDrafts;
  onSaved: (ticket: number, info: ReviewsInfo) => void;
}) {
  const status = effectiveStatus(experiment, info.followups);
  const followups = followUpsOf(experiment.id, info.followups);
  const origin = info.reports.find((r) => r.id === experiment.origin_report_id) ?? null;
  const pattern = info.projection?.patterns.find((p) => p.id === experiment.pattern_id) ?? null;

  return (
    <div className="min-w-0" data-testid="reviews-experiment-detail">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <h3 className="min-w-0 text-[14px] font-semibold">{experiment.title}</h3>
        <Badge tone={statusTone(status)}>{statusLabel(status)}</Badge>
      </div>
      <p className="mt-1 text-[12.5px] text-muted">
        Origin:{" "}
        {origin ? (
          <ReportLink report={origin} label={`${shortDay(origin.date)} ${origin.kind} review`} open={open} />
        ) : (
          `${shortDay(experiment.origin_date)} review`
        )}
        {experiment.next_check && ` · Next check: ${shortDay(experiment.next_check)}`}
      </p>
      <dl className="mt-4 grid grid-cols-[110px_minmax(0,1fr)] gap-x-4 gap-y-2 text-[12.5px]">
        {(
          [
            ["Experiment", experiment.experiment],
            ["Expected benefit", experiment.expected_benefit],
            ["Success measure", experiment.success_measure],
            ["Effort", experiment.effort],
          ] as const
        )
          .filter(([, value]) => value)
          .map(([label, value]) => (
            <div key={label} className="contents">
              <dt className="text-muted">{label}</dt>
              <dd>{value}</dd>
            </div>
          ))}
      </dl>

      <SectionHead title="Decisions & results" />
      <ol className="ml-1 border-l border-line" data-testid="reviews-timeline">
        <TimelineItem title={`${shortDay(experiment.origin_date)} · Agent proposal`}>
          Suggested in the {origin?.kind ?? "review"} report. No adoption inferred.
        </TimelineItem>
        {experiment.status !== "proposed" && (
          <TimelineItem title={`From the reports · ${statusLabel(experiment.status)}`}>
            Derived by the analysis from the conversations below, not recorded by you.
          </TimelineItem>
        )}
        {followups.map((f) => (
          <TimelineItem
            key={f.id}
            title={`${shortDay(f.recorded_at)} · ${f.status ? `Your update · ${statusLabel(f.status)}` : "Your note"}`}
            testId="reviews-followup"
          >
            <span className="whitespace-pre-line">“{f.note}”</span>
            {f.source && (
              <span className="ml-1.5">
                <SourceLink evidence={f.source} label={`Source · ${shortDay(f.source.at)}`} open={open} />
              </span>
            )}
          </TimelineItem>
        ))}
      </ol>

      {experiment.evidence.length > 0 && (
        <>
          <SectionHead title="Supporting conversations" />
          <ul className="space-y-1.5 text-[12.5px]">
            {experiment.evidence.map((e) => (
              <li key={e.id}>
                <SourceLink evidence={e} label={`${e.workspace} · ${shortDay(e.at)}`} open={open} />
                {e.quote && <span className="ml-2 text-muted">{e.quote}</span>}
              </li>
            ))}
          </ul>
        </>
      )}

      <FollowUpForm experiment={experiment} status={status} sources={sourceChoices(experiment, pattern)} gate={gate} drafts={drafts} onSaved={onSaved} />
    </div>
  );
}

function TimelineItem({ title, testId, children }: { title: string; testId?: string; children: ReactNode }) {
  return (
    <li className="relative pb-3 pl-5 last:pb-0" data-testid={testId}>
      <span className="absolute -left-[3.5px] top-[6px] h-1.5 w-1.5 rounded-full bg-muted" />
      <div className="text-[12.5px] font-medium">{title}</div>
      <div className="mt-0.5 text-[12.5px] text-muted">{children}</div>
    </li>
  );
}

function FollowUpForm({
  experiment,
  status,
  sources,
  gate,
  drafts,
  onSaved,
}: {
  experiment: ReviewExperiment;
  status: ReviewExperiment["status"];
  sources: ReviewEvidence[];
  gate: SnapshotGate;
  drafts: FollowUpDrafts;
  onSaved: (ticket: number, info: ReviewsInfo) => void;
}) {
  const id = experiment.id;
  // The page's store holds the draft; this state only re-renders the form when it changes.
  const [draft, setDraft] = useState<FollowUpDraft>(() => drafts.draft(id));
  const [failure, setFailure] = useState<string | null>(() => drafts.failure(id));
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const edit = (change: Partial<FollowUpDraft>) => {
    const next = { ...drafts.draft(id), ...change };
    drafts.edit(id, next);
    setDraft(next);
    setProblem(null);
  };

  async function save() {
    if (gate.mutating) return;
    const invalid = followUpProblem(draft);
    if (invalid) return setProblem(invalid);
    const submission = drafts.submit(id, () => crypto.randomUUID());
    setSaving(true);
    setFailure(null);
    const ticket = gate.beginMutation();
    try {
      const next = await client.call("reviews.follow_up", {
        experiment_id: id,
        status: submission.status,
        note: submission.draft.note.trim(),
        source_seq: submission.draft.sourceSeq,
        idempotency_key: submission.key,
      });
      onSaved(ticket, next);
      drafts.saved(id, submission);
      setDraft(drafts.draft(id));
      toast(`${submission.status ? "Follow-up" : "Note"} saved for ${experiment.title}`);
    } catch (error) {
      drafts.failed(id, errorMessage(error));
      setFailure(errorMessage(error));
      toast(`Could not save the follow-up: ${errorMessage(error)}`, "error");
    } finally {
      gate.endMutation();
      setSaving(false);
    }
  }

  const choiceOptions: Array<SelectOption<FollowUpChoice>> = [
    { value: "keep", label: `Note only (status stays ${statusLabel(status)})`, description: "Defer, decline or add context; records no status." },
    ...CHOICES,
  ];
  const sourceOptions: Array<SelectOption<string>> = [
    { value: "", label: "No source conversation" },
    ...sources.map((e) => ({ value: String(e.source_seq), label: `${e.workspace} · ${shortDay(e.at)}`, description: e.quote || e.branch })),
  ];

  return (
    <form
      className="mt-7 border-t border-line pt-5"
      data-testid="reviews-followup-form"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <h3 className="text-[13px] font-semibold">Record a decision or outcome</h3>
      <p className="mt-1 text-[12px] text-muted">
        Your note is the evidence for this update, saved with today's date. A note only records no status; no status advances automatically.
      </p>
      <div className="mt-3 flex flex-col gap-3">
        <label className="flex flex-col gap-1">
          <span className="text-[12px] font-medium text-muted">Update type</span>
          <Select
            value={draft.choice}
            options={choiceOptions}
            onChange={(choice) => edit({ choice })}
            title="Update type"
            testId="reviews-followup-type"
            width={320}
            className="h-7 max-w-[320px] border border-line"
          />
        </label>
        <label className="flex flex-col gap-1">
          <span className="text-[12px] font-medium text-muted">Decision, result, or evidence</span>
          <textarea
            value={draft.note}
            onChange={(event) => edit({ note: event.target.value })}
            placeholder={
              draft.choice === "observed_effect"
                ? "What you measured, over which sessions and dates (e.g. 1 CI rework over 3 reviews, Nov 9–12)."
                : "What did you decide or observe? Add dates if you have them."
            }
            rows={3}
            className={cx(inputClass, "h-auto resize-y py-1.5 leading-relaxed")}
            data-testid="reviews-followup-note"
          />
        </label>
        {sources.length > 0 && (
          <label className="flex flex-col gap-1">
            <span className="text-[12px] font-medium text-muted">
              Source conversation{draft.choice === "observed_effect" ? " (required for an observed effect)" : ""}
            </span>
            <Select
              value={draft.sourceSeq == null ? "" : String(draft.sourceSeq)}
              options={sourceOptions}
              onChange={(value) => edit({ sourceSeq: value ? Number(value) : null })}
              title="Source conversation"
              testId="reviews-followup-source"
              width={360}
              className="h-7 max-w-[360px] border border-line"
            />
          </label>
        )}
        {draft.choice === "observed_effect" && sources.length === 0 && (
          <p className="text-[12px] text-attention">No cited conversation to attach: an observed effect cannot be recorded for this experiment yet.</p>
        )}
        {(problem || failure) && (
          <p role="alert" className="text-[12px] text-danger" data-testid="reviews-followup-problem">
            {problem ?? `Not saved: ${failure}. Your note is kept; saving again sends the same update once.`}
          </p>
        )}
      </div>
      <div className="mt-3 flex flex-wrap items-center justify-between gap-2">
        <span className="text-[12px] text-muted">{experiment.next_check ? `${shortDay(experiment.next_check)} · Compare the next eligible sessions.` : ""}</span>
        <Button type="submit" variant="primary" busy={saving} data-testid="reviews-followup-save">
          Save update
        </Button>
      </div>
    </form>
  );
}

function History({ info, open }: { info: ReviewsInfo; open: OpenSource }) {
  const reports = reportsNewestFirst(info.reports);
  const missing = weeksWithoutReports(info.reports, info.coverage_start, info.coverage_end);
  const shortened = info.reports.filter((r) => r.shortened);
  return (
    <div data-testid="reviews-history">
      <PageTitle title="Report history" subtitle="Supporting conversations, including archived reports and follow-up decisions.">
        <Button onClick={() => openSettings(true, "daily-review")} data-testid="reviews-settings">
          <Settings size={13} /> Daily review settings
        </Button>
      </PageTitle>
      <div className="rounded-lg border border-line bg-panel px-5 py-4 text-[12.5px]" data-testid="reviews-coverage">
        <div className="text-[13px] font-medium">Coverage of these insights</div>
        <p className="mt-1.5 text-muted">
          {info.coverage_start && info.coverage_end ? dayRange(info.coverage_start, info.coverage_end) : "No covered period yet"}
          {` · ${plural(info.reports.length, "report")}`}
          {missing.length > 0 && ` · No report for ${missing.join(", ")}`}
          {shortened.length > 0 && ` · ${shortened.map((r) => shortDay(r.date)).join(", ")} shortened`}
        </p>
        {info.projection && <SourceCoverageNote coverage={coverageSummary(info.projection.source_coverage)} className="mt-1" />}
        <p className="mt-1 text-muted">
          Daily analyses read the 7 prior calendar days and the latest weekly synthesis within 28 days. Weekly analyses read 4 weeks. These windows are fixed.
        </p>
      </div>

      <SectionHead
        title="Reports"
        aside={reports.length ? `${dayRange(reports.at(-1)?.date ?? "", reports[0]?.date ?? "")} · All review conversations` : undefined}
      />
      {reports.length ? (
        <div className="divide-y divide-line overflow-hidden rounded-lg border border-line">
          {reports.map((r) => (
            <div key={r.id} className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3" data-testid="reviews-report-row">
              <div className="w-16 shrink-0 text-[12.5px]">{shortDay(r.date)}</div>
              <div className="min-w-0 flex-1 basis-56">
                <div className="text-[13px] font-medium">{r.title}</div>
                {r.summary && <div className="mt-0.5 text-[12px] text-muted">{r.summary}</div>}
              </div>
              <div className="flex shrink-0 flex-wrap gap-1">
                <Badge tone={reportStatusTone(r.status)}>{reportStatusLabel(r.status)}</Badge>
                {r.archived && <Badge>Archived</Badge>}
                {r.shortened && <Badge tone="attention" title="The conversation was too long and its earlier messages were left out">Shortened</Badge>}
              </div>
              <div className="w-40 shrink-0 text-[12.5px]">
                <ReportLink report={r} open={open} />
              </div>
            </div>
          ))}
        </div>
      ) : (
        <EmptyState title="No review reports yet">Daily reviews appear here once they have run.</EmptyState>
      )}
      <p className="mt-4 text-[12px] leading-relaxed text-muted">
        A missing report can mean no activity, a deletion or a review that never started. Incomplete and shortened conversations keep their limitations.
      </p>
    </div>
  );
}

/** How much of the original sessions the analysis read, so a count or a trend never implies all of them. */
function SourceCoverageNote({ coverage, className }: { coverage: CoverageSummary; className?: string }) {
  if (!coverage.available) {
    return <p className={cx("text-[12px] text-muted", className)} data-testid="reviews-source-coverage">Original event coverage is unavailable; these observations cannot establish complete counts or an improvement.</p>;
  }
  return (
    <p className={cx("text-[12px] text-muted", className)} data-testid="reviews-source-coverage">
      Original events read: {coverage.included} of {coverage.available}
      {coverage.complete
        ? ", every day in full."
        : coverage.sampledDays.length
          ? `. Only a sample was read on ${coverage.sampledDays.slice(0,4).map(shortDay).join(", ") + (coverage.sampledDays.length>4 ? ` and ${coverage.sampledDays.length-4} other days` : "")}: counts there are lower bounds.`
          : "."}
      {coverage.unknownDays > 0 && ` No original events are available for ${coverage.unknownDays} days; their coverage is unknown.`}
    </p>
  );
}
