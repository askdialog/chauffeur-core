// The workspace's repos and their pull requests, in the header: one chip per repo on one line, then
// one per PR its agents opened from their own worktrees (one per sub-ticket, say); a chip with a PR
// opens it on GitHub. What does not fit collapses into a `+N` chip whose popover lists them all.
// A PR that can be merged on its own gets a green Merge button beside it.

import type { PrInfo } from "@chauffeur/protocol";
import { AlertTriangle, CheckCircle2, CircleDashed, CircleX, GitMerge, GitPullRequest, RefreshCw } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { fitCount } from "@/lib/fit";
import { openExternal } from "@/lib/native";
import { mergeBlocker } from "@/lib/prs";
import { Button, cx, IconButton, Spinner } from "./ui";

export type PrStatus = { prs: PrInfo[] | null; error: string | null; loading: boolean; refresh: () => void };

/** A chip: a repo of the workspace, or a PR of an agent's worktree. */
export type PrEntry = { key: string; label: string; pr: PrInfo | undefined };

/** The workspace's repos (their PR, if any), then the PRs its agents opened on their branches. */
export function prEntries(repos: string[], prs: PrInfo[] | null): PrEntry[] {
  const own = repos.map((repo) => ({ key: repo, label: repo, pr: prs?.find((pr) => pr.repo === repo && !pr.group) }));
  const agents = (prs ?? [])
    .filter((pr) => pr.group && pr.number != null)
    .map((pr) => ({ key: pr.branch, label: `${pr.group} · ${pr.repo}`, pr }));
  return [...own, ...agents];
}

const GAP = 4;
const OVERFLOW_WIDTH = 52;
const REFRESH_WIDTH = 22;

export function PrRow({
  repos,
  status,
  refreshable,
  onMerge,
}: {
  repos: string[];
  status: PrStatus;
  refreshable: boolean;
  /** Offers to merge each PR that can be merged on its own; none, no Merge button. */
  onMerge?: (pr: PrInfo) => void;
}) {
  const row = useRef<HTMLDivElement>(null);
  const measure = useRef<HTMLDivElement>(null);
  const entries = prEntries(repos, status.prs);
  const mergeOf = (pr: PrInfo | undefined) =>
    onMerge && pr && !mergeBlocker(pr, status.prs ?? []) ? () => onMerge(pr) : undefined;
  // Why an open PR has no Merge button, for its chip's tooltip.
  const blockerOf = (pr: PrInfo | undefined) =>
    onMerge && pr?.state === "OPEN" ? mergeBlocker(pr, status.prs ?? []) : null;
  const [layout, setLayout] = useState<{ fit: number; titleWidth: number | null }>({ fit: entries.length, titleWidth: null });

  // Measure every chip off screen, then lay them out on one line; again whenever the row resizes.
  useLayoutEffect(() => {
    const compute = () => {
      if (!row.current || !measure.current) return;
      const chips = Array.from(measure.current.children, (child) => {
        const chip = child as HTMLElement;
        const title = chip.querySelector<HTMLElement>("[data-chip-title]");
        return { full: chip.offsetWidth, bare: title ? chip.offsetWidth - title.offsetWidth - GAP : chip.offsetWidth, titled: !!title };
      });
      const available = row.current.clientWidth - (refreshable ? REFRESH_WIDTH + GAP : 0);
      setLayout(oneLine(chips, available));
    };
    compute();
    const observer = new ResizeObserver(compute);
    if (row.current) observer.observe(row.current);
    return () => observer.disconnect();
  }, [repos, status.prs, refreshable, !!onMerge]);

  const { fit, titleWidth } = layout;
  const hidden = entries.slice(fit);
  return (
    <div ref={row} className="relative flex min-w-[200px] flex-1 items-center gap-1" data-testid="pr-row">
      <div ref={measure} aria-hidden className="pointer-events-none invisible absolute top-0 left-0 flex gap-1 whitespace-nowrap">
        {entries.map((entry) => (
          <RepoChip key={entry.key} repo={entry.label} pr={entry.pr} onMerge={mergeOf(entry.pr)} measuring />
        ))}
      </div>
      {entries.slice(0, fit).map((entry) => (
        <RepoChip
          key={entry.key}
          repo={entry.label}
          pr={entry.pr}
          titleWidth={titleWidth}
          onMerge={mergeOf(entry.pr)}
          blocker={blockerOf(entry.pr)}
        />
      ))}
      {hidden.length > 0 && <OverflowChip entries={entries} hidden={hidden} all={fit === 0} mergeOf={mergeOf} blockerOf={blockerOf} />}
      {refreshable && (
        <IconButton
          title={status.error ? `Could not read the PR status: ${status.error}. Retry` : "Refresh the PR status"}
          onClick={status.refresh}
          disabled={status.loading}
          className="h-[18px] w-[18px] shrink-0"
        >
          {status.loading ? (
            <Spinner size={10} />
          ) : status.error ? (
            <AlertTriangle size={11} className="text-danger" />
          ) : (
            <RefreshCw size={10} />
          )}
        </IconButton>
      )}
    </div>
  );
}

/** A title shorter than this says nothing: below it, chips show none. */
const MIN_TITLE = 60;

/**
 * Every chip with its full title when they all fit; else as many as fit without titles beside the
 * +N chip, the space left shared by their titles (none when the share is too small to read).
 */
function oneLine(
  chips: Array<{ full: number; bare: number; titled: boolean }>,
  available: number,
): { fit: number; titleWidth: number | null } {
  const full = fitCount(
    chips.map((c) => c.full),
    available,
    GAP,
    OVERFLOW_WIDTH,
  );
  if (full === chips.length) return { fit: full, titleWidth: null };
  const fit = fitCount(
    chips.map((c) => c.bare),
    available,
    GAP,
    OVERFLOW_WIDTH,
  );
  const shown = chips.slice(0, fit);
  const used =
    shown.reduce((sum, c) => sum + c.bare, 0) + GAP * Math.max(0, fit - 1) + (fit < chips.length ? GAP + OVERFLOW_WIDTH : 0);
  const titled = shown.filter((c) => c.titled).length;
  const share = titled > 0 ? Math.floor((available - used) / titled) - GAP : 0;
  return { fit, titleWidth: share >= MIN_TITLE ? share : 0 };
}

const CHIP_SHAPE =
  "inline-flex max-w-[380px] shrink-0 items-center gap-1 rounded border bg-raised px-1.5 text-[11.5px] leading-[18px]";
const CHIP = `${CHIP_SHAPE} border-line text-muted`;

/**
 * A repo of the workspace. With a pull request, the chip is the PR: number, title and state,
 * opened on GitHub in one click. The tooltip tells the branch and what is not pushed.
 */
function RepoChip({
  repo,
  pr,
  titleWidth = null,
  onMerge,
  blocker = null,
  measuring = false,
}: {
  repo: string;
  pr: PrInfo | undefined;
  titleWidth?: number | null;
  onMerge?: () => void;
  /** Why it has no Merge button, told in the tooltip. */
  blocker?: string | null;
  /** The off-screen copy that measures the chip: no test handles. */
  measuring?: boolean;
}) {
  const local = pr ? localState(pr) : `${repo}: reading the pull request status…`;
  const marks = (
    <>
      {pr && pr.ahead > 0 && <span className="shrink-0 text-faint">↑{pr.ahead}</span>}
      {pr?.dirty && <span className="shrink-0 text-attention">●</span>}
    </>
  );
  if (pr?.number != null && pr.url) {
    const url = pr.url;
    const chip = (
      <button
        title={`Open #${pr.number} on GitHub${pr.title ? `: ${pr.title}` : ""}\n${local}${blocker ? `\nNot mergeable yet: ${blocker}` : ""}`}
        onClick={() => void openExternal(url)}
        data-repo-chip={repo}
        className={cx(CHIP, "hover:border-line-strong hover:text-fg")}
      >
        <span className="shrink-0">{repo}</span>
        {marks}
        <span className="inline-flex shrink-0 items-center gap-0.5 text-fg/90">
          <GitPullRequest size={10} />#{pr.number}
        </span>
        {pr.title && titleWidth !== 0 && (
          <span data-chip-title className="min-w-0 truncate text-faint" style={titleWidth ? { maxWidth: titleWidth } : undefined}>
            {pr.title}
          </span>
        )}
        <PrState pr={pr} />
      </button>
    );
    return onMerge ? (
      <span className="inline-flex shrink-0 items-center gap-1">
        {chip}
        <MergeButton pr={pr} onMerge={onMerge} measuring={measuring} />
      </span>
    ) : (
      chip
    );
  }
  return (
    <span title={local} data-repo-chip={repo} className={CHIP}>
      <span className="shrink-0">{repo}</span>
      {marks}
      {pr && <span className="shrink-0 text-faint">no PR</span>}
    </span>
  );
}

/** The green button that merges one PR (after its dialog: the method, then confirm). */
function MergeButton({ pr, onMerge, measuring = false }: { pr: PrInfo; onMerge: () => void; measuring?: boolean }) {
  return (
    <Button
      size="xs"
      variant="success"
      title={`Merge #${pr.number}`}
      onClick={onMerge}
      data-testid={measuring ? undefined : "merge"}
      data-merge={measuring ? undefined : pr.branch}
      data-merge-repo={measuring ? undefined : pr.repo}
      className="shrink-0"
    >
      <GitMerge size={10} /> Merge
    </Button>
  );
}

/**
 * `+N`, or `N repos` when none fits: hovering, focusing or clicking it shows every repo. It is red
 * when a hidden PR's checks fail.
 */
function OverflowChip({
  entries,
  hidden,
  all,
  mergeOf,
  blockerOf,
}: {
  entries: PrEntry[];
  hidden: PrEntry[];
  all: boolean;
  mergeOf: (pr: PrInfo | undefined) => (() => void) | undefined;
  blockerOf: (pr: PrInfo | undefined) => string | null;
}) {
  const anchor = useRef<HTMLSpanElement>(null);
  const [hovered, setHovered] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [alignRight, setAlignRight] = useState(true);
  const leaving = useRef<ReturnType<typeof setTimeout> | null>(null);
  const open = hovered || pinned;
  const failing = hidden.some((entry) => entry.pr?.checks === "FAILURE" && entry.pr?.state === "OPEN");

  const show = () => {
    if (leaving.current) clearTimeout(leaving.current);
    const box = anchor.current?.getBoundingClientRect();
    if (box) setAlignRight(box.left + POPOVER_WIDTH > window.innerWidth - 8);
    setHovered(true);
  };
  // A short grace period lets the pointer cross the gap down to the popover.
  const hide = () => {
    leaving.current = setTimeout(() => setHovered(false), 150);
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      setPinned(false);
      setHovered(false);
    };
    const onDown = (event: MouseEvent) => {
      if (!anchor.current?.contains(event.target as Node)) {
        setPinned(false);
        setHovered(false);
      }
    };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("mousedown", onDown);
    };
  }, [open]);
  useEffect(() => () => void (leaving.current && clearTimeout(leaving.current)), []);

  return (
    <span ref={anchor} className="relative shrink-0" onMouseEnter={show} onMouseLeave={hide}>
      <button
        type="button"
        onClick={() => {
          show();
          setPinned((p) => !p);
        }}
        onFocus={show}
        aria-expanded={open}
        title={failing ? `${entries.length} chips: checks fail on a PR not shown here` : `${entries.length} repos and PRs`}
        data-testid="pr-overflow"
        className={cx(
          CHIP_SHAPE,
          failing ? "border-danger/50 text-danger hover:border-danger" : "border-line text-muted hover:border-line-strong hover:text-fg",
        )}
      >
        {failing && <CircleX size={10} className="shrink-0" />}
        {all ? `${entries.length} repos` : `+${hidden.length}`}
      </button>
      {open && <PrPopover entries={entries} alignRight={alignRight} mergeOf={mergeOf} blockerOf={blockerOf} />}
    </span>
  );
}

const POPOVER_WIDTH = 560;

/** Every repo of the workspace: its PR (opened in one click) and what is not pushed. */
function PrPopover({
  entries,
  alignRight,
  mergeOf,
  blockerOf,
}: {
  entries: PrEntry[];
  alignRight: boolean;
  mergeOf: (pr: PrInfo | undefined) => (() => void) | undefined;
  blockerOf: (pr: PrInfo | undefined) => string | null;
}) {
  return (
    <div
      data-popover
      data-testid="pr-popover"
      style={{ width: POPOVER_WIDTH }}
      className={cx(
        "absolute top-full z-30 mt-1 max-h-[60vh] overflow-y-auto rounded-lg border border-line-strong bg-raised py-1 shadow-xl",
        alignRight ? "right-0" : "left-0",
      )}
    >
      {entries.map(({ key, label: repo, pr }) => {
        const url = pr?.url;
        const blocker = blockerOf(pr);
        const body = (
          <>
            {/* The title has a line of its own: a long worktree label beside it would leave it no width. */}
            <div className="flex items-baseline gap-2">
              <span title={repo} className="min-w-0 truncate font-medium text-fg">
                {repo}
              </span>
              {pr?.number != null ? (
                <>
                  <span className="inline-flex shrink-0 items-center gap-0.5 text-fg/90">
                    <GitPullRequest size={11} />#{pr.number}
                  </span>
                  <span className="ml-auto inline-flex shrink-0 items-center gap-1 text-[11px] text-faint">
                    <PrState pr={pr} />
                    {checksLabel(pr)}
                  </span>
                </>
              ) : (
                <span className="shrink-0 text-faint">{pr ? "no PR" : "…"}</span>
              )}
            </div>
            {pr?.title && <div className="mt-0.5 line-clamp-2 text-muted">{pr.title}</div>}
            {pr && <div className="mt-0.5 font-mono text-[11px] text-faint">{localState(pr)}</div>}
            {blocker && <div className="mt-0.5 text-[11px] text-faint">Not mergeable yet: {blocker}</div>}
          </>
        );
        const merge = mergeOf(pr);
        return url ? (
          <div key={key} className="flex items-center gap-2 pr-3 hover:bg-hover">
            <button
              type="button"
              onClick={() => void openExternal(url)}
              className="block min-w-0 flex-1 py-1.5 pl-3 text-left text-[12.5px]"
            >
              {body}
            </button>
            {pr && merge && <MergeButton pr={pr} onMerge={merge} />}
          </div>
        ) : (
          <div key={key} className="px-3 py-1.5 text-[12.5px]">
            {body}
          </div>
        );
      })}
    </div>
  );
}

/** `tester/xyz-1 ← staging · 2 commits ahead · uncommitted changes` */
function localState(pr: PrInfo): string {
  const parts = [`${pr.branch} ← ${pr.base_branch}`];
  parts.push(pr.ahead > 0 ? `${pr.ahead} commit${pr.ahead === 1 ? "" : "s"} ahead` : "no commit");
  if (pr.dirty) parts.push("uncommitted changes");
  return parts.join(" · ");
}

function checksLabel(pr: PrInfo): string | null {
  if (pr.state !== "OPEN" || pr.is_draft) return null;
  if (pr.checks === "SUCCESS") return "checks passed";
  if (pr.checks === "FAILURE") return "checks failed";
  if (pr.checks === "PENDING") return "checks running";
  return null;
}

function PrState({ pr }: { pr: PrInfo }) {
  if (pr.state === "MERGED") return <span className="shrink-0 text-accent">merged</span>;
  if (pr.state === "CLOSED") return <span className="shrink-0 text-faint">closed</span>;
  if (pr.is_draft) return <span className="shrink-0 text-faint">draft</span>;
  if (pr.checks === "SUCCESS") return <CheckCircle2 size={10} className="shrink-0 text-success" aria-label="checks passed" />;
  if (pr.checks === "FAILURE") return <CircleX size={10} className="shrink-0 text-danger" aria-label="checks failed" />;
  if (pr.checks === "PENDING") return <CircleDashed size={10} className="shrink-0 text-attention" aria-label="checks running" />;
  return null;
}
