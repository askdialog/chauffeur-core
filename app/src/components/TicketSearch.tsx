import type { LinearIssue, LinearMilestone, TicketRef } from "@chauffeur/protocol";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { isTicketIdentifier } from "@/lib/tickets";
import { errorMessage } from "@/lib/rpc";
import { client } from "@/state/app";
import { cx } from "./ui";

export type TicketSearch = ReturnType<typeof useTicketSearch>;

/** A row of the results: a ticket, or a milestone when they are asked for. */
export type TicketOption = { kind: "ticket"; issue: LinearIssue } | { kind: "milestone"; milestone: LinearMilestone };

/**
 * Linear's tickets to pick from: yours (open ones) once the list opens, any ticket once you type,
 * without those `attached`; with `milestones`, the project milestones after them (the next ones of
 * your projects, or those named like the search, or the one a pasted link names). Enter picks the
 * highlighted row, or attaches a typed identifier as is.
 */
export function useTicketSearch(
  attached: ReadonlySet<string>,
  onPick: (ticket: TicketRef, repos: string[], issue: LinearIssue | null) => void,
  milestones?: { attached: string | null; onPick: (milestone: LinearMilestone) => void },
) {
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState(false);
  const [mine, setMine] = useState<LinearIssue[] | null>(null);
  const [results, setResults] = useState<LinearIssue[] | null>(null);
  const [mineMilestones, setMineMilestones] = useState<LinearMilestone[]>([]);
  const [foundMilestones, setFoundMilestones] = useState<LinearMilestone[]>([]);
  const withMilestones = milestones != null;
  const [loading, setLoading] = useState(false);
  const [linearError, setLinearError] = useState<string | null>(null);
  const [highlight, setHighlight] = useState(0);
  const generation = useRef(0);
  const q = query.trim();

  useEffect(() => {
    if (!open || mine !== null) return;
    const current = ++generation.current;
    setLoading(true);
    client
      .call("linear.search", { mine: true, limit: 30, milestones: withMilestones })
      .then((result) => {
        setMine(result.issues);
        setMineMilestones(result.milestones);
        setLinearError(null);
      })
      .catch((error) => {
        setMine([]);
        setLinearError(errorMessage(error));
      })
      .finally(() => {
        if (current === generation.current) setLoading(false);
      });
  }, [open, mine]);

  useEffect(() => {
    if (!q) {
      setResults(null);
      setFoundMilestones([]);
      return;
    }
    const current = ++generation.current;
    const timer = setTimeout(async () => {
      setLoading(true);
      try {
        const result = await client.call("linear.search", { query: q, mine: false, limit: 20, milestones: withMilestones });
        if (current === generation.current) {
          setResults(result.issues);
          setFoundMilestones(result.milestones);
          setLinearError(null);
        }
      } catch (error) {
        if (current === generation.current) {
          setResults([]);
          setLinearError(errorMessage(error));
        }
      } finally {
        if (current === generation.current) setLoading(false);
      }
    }, 200);
    return () => clearTimeout(timer);
  }, [q]);

  const list = (q ? (results ?? []) : (mine ?? [])).filter((issue) => !attached.has(issue.identifier));
  const milestoneList = (q ? foundMilestones : mineMilestones).filter((m) => m.id !== milestones?.attached);
  const options: TicketOption[] = [
    ...list.map((issue) => ({ kind: "ticket" as const, issue })),
    ...milestoneList.map((milestone) => ({ kind: "milestone" as const, milestone })),
  ];
  useEffect(() => setHighlight(0), [q, options.length]);

  const pick = (issue: LinearIssue) => {
    onPick({ identifier: issue.identifier, title: issue.title, url: issue.url }, issue.repos, issue);
    setQuery("");
  };

  const pickMilestone = (milestone: LinearMilestone) => {
    milestones?.onPick(milestone);
    setQuery("");
  };

  const pickOption = (option: TicketOption) =>
    option.kind === "ticket" ? pick(option.issue) : pickMilestone(option.milestone);

  const addTyped = () => {
    if (!isTicketIdentifier(q)) return false;
    onPick({ identifier: q.toUpperCase(), title: null, url: null }, [], null);
    setQuery("");
    return true;
  };

  /** Arrows move in the list; Enter picks. */
  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.metaKey || e.ctrlKey) return;
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setOpen(true);
      setHighlight((h) => Math.min(h + 1, Math.max(options.length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setHighlight((h) => Math.max(h - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (open && options[highlight]) pickOption(options[highlight]);
      else addTyped();
    }
  };

  return {
    query,
    setQuery,
    open,
    setOpen,
    q,
    list,
    milestones: milestoneList,
    options,
    loading,
    linearError,
    highlight,
    setHighlight,
    pick,
    pickOption,
    onKeyDown,
  };
}

/** The tickets `search` offers, with what to do when there is none. */
export function TicketResults({ search, className }: { search: TicketSearch; className?: string }) {
  const { q, list, loading, linearError, highlight } = search;
  return (
    <div className={className} data-testid="ticket-results">
      <div className="flex items-center justify-between border-b border-line px-2.5 py-1 text-[10.5px] font-medium uppercase tracking-wide text-faint">
        <span>{q ? "All tickets" : "Assigned to you"}</span>
        {!q && <span className="normal-case tracking-normal">type to search any ticket</span>}
      </div>
      <div className="max-h-[240px] overflow-y-auto py-0.5">
        {list.map((issue, index) => (
          <button
            type="button"
            key={issue.identifier}
            onMouseDown={(e) => {
              e.preventDefault();
              search.pick(issue);
            }}
            onMouseEnter={() => search.setHighlight(index)}
            className={cx("flex w-full min-w-0 items-center gap-2 px-2.5 py-1.5 text-left", index === highlight && "bg-hover")}
            data-ticket={issue.identifier}
          >
            <StateDot state={issue.state} />
            <span className="w-[72px] shrink-0 font-medium text-muted">{issue.identifier}</span>
            <span className="min-w-0 flex-1 truncate text-fg">{issue.title}</span>
            {issue.repos.length > 0 && (
              <span className="max-w-[180px] shrink-0 truncate text-[11px] text-faint">{issue.repos.join(", ")}</span>
            )}
            {issue.state && <span className="shrink-0 text-[11px] text-faint">{issue.state}</span>}
          </button>
        ))}
        {list.length === 0 && (
          <div className="px-2.5 py-2 text-[12px] text-faint">
            {loading
              ? "Searching Linear…"
              : linearError
                ? isTicketIdentifier(q)
                  ? `Linear is unavailable (${linearError}). Press ↵ to attach ${q.toUpperCase()}.`
                  : `Linear is unavailable: ${linearError}`
                : q
                  ? `No ticket matches “${q}”.`
                  : "No open ticket is assigned to you."}
          </div>
        )}
      </div>
    </div>
  );
}

/** Linear's workflow states, by name: a colour hint only. */
export function StateDot({ state }: { state: string | null }) {
  const name = (state ?? "").toLowerCase();
  const colour = name.includes("progress")
    ? "bg-attention"
    : name.includes("review")
      ? "bg-accent"
      : name.includes("done") || name.includes("merged")
        ? "bg-success"
        : name.includes("cancel") || name.includes("duplicate")
          ? "bg-faint"
          : "border border-line-strong";
  return <span className={cx("h-2 w-2 shrink-0 rounded-full", colour)} aria-hidden />;
}
