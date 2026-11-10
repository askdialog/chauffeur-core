import type { PrInfo, TicketRef, WorkspaceInfo } from "@chauffeur/protocol";
import {
  AlertTriangle,
  Archive,
  ArchiveRestore,
  Copy,
  GitBranch,
  GitPullRequest,
  Lock,
  MoreHorizontal,
  Plus,
  RefreshCw,
  ScrollText,
  Sparkles,
  Telescope,
  X as XIcon,
} from "lucide-react";
import { useEffect, useState } from "react";
import { workspaceName, workspaceRepoNames, workspaceRepos } from "@/lib/format";
import { openExternal } from "@/lib/native";
import { errorMessage, isNotImplemented } from "@/lib/rpc";
import { ticketsCollapsed, ticketsEditable, ticketTooltip, ticketUnread, withoutTicket, withTicket } from "@/lib/tickets";
import type { LogEntry } from "@/lib/workspaceLive";
import { type RemoteOpen, watchRemote } from "@/lib/workers";
import { client, loadRepos, toast, upsertWorkspace, useApp } from "@/state/app";
import { useWorkspaceSelector, useWorkspaceSession, type WorkspaceSession } from "@/state/session";
import { OpenButton } from "./OpenButton";
import { PrRow, type PrStatus } from "./PrChips";
import { PromoteDialog } from "./PromoteDialog";
import { HistoryButtons } from "./HistoryButtons";
import { ShowSidebarButton } from "./ShowSidebarButton";
import { ArchiveDialog, archiveWorkspace } from "./ArchiveDialog";
import { TransferBanner } from "./TransferBanner";
import { CreatePrDialog, MergeDialog } from "./PrDialogs";
import { TicketResults, useTicketSearch } from "./TicketSearch";
import { Badge, Button, cx, IconButton, inputClass, Modal, Spinner, StatusDot, workspaceStatus } from "./ui";
import { WorkspaceViews } from "./WorkspaceViews";

export function WorkspaceView({ workspace }: { workspace: WorkspaceInfo }) {
  const session = useWorkspaceSession(workspace.id);

  return (
    <div className="flex h-full min-w-0 flex-1 flex-col">
      <Header workspace={workspace} session={session} />
      <WorkspaceViews
        workspace={workspace}
        session={session}
        setup={<SetupProgress workspace={workspace} session={session} />}
      />
    </div>
  );
}

function Header({ workspace, session }: { workspace: WorkspaceInfo; session: WorkspaceSession }) {
  const repos = workspace.state === "archived" ? workspaceRepoNames(workspace) : workspaceRepos(workspace);
  const prs = usePrStatus(workspace, session);
  const status = workspaceStatus(workspace);
  const [archiveOpen, setArchiveOpen] = useState(false);
  const [archiveRefusal, setArchiveRefusal] = useState<string | null>(null);
  // No confirmation: a workspace can be restored; the dialog only says why the daemon refused.
  const archive = async () => {
    const refused = await archiveWorkspace(workspace);
    if (refused) {
      setArchiveRefusal(refused);
      setArchiveOpen(true);
    }
  };
  const [prOpen, setPrOpen] = useState(false);
  const [merging, setMerging] = useState<PrInfo | null>(null);
  const [promoteOpen, setPromoteOpen] = useState(false);
  const [restoring, setRestoring] = useState(false);

  const restore = async () => {
    setRestoring(true);
    try {
      upsertWorkspace(await client.call("workspace.restore", { workspace: workspace.id }));
      toast(`${workspaceName(workspace)} restored`);
    } catch (error) {
      toast(`Could not restore: ${errorMessage(error)}`, "error");
    } finally {
      setRestoring(false);
    }
  };

  // A workspace moved to a GCP worker opens there over SSH, or says why not yet (asked again when the
  // window comes back); one moved to a machine declared by hand, nowhere from here.
  const [remote, setRemote] = useState<RemoteOpen | null>(null);
  useEffect(() => {
    setRemote(null);
    const machine = workspace.moved_to;
    if (!machine) return;
    return watchRemote(
      machine,
      () => client.call("worker.list", {}),
      setRemote,
      (error) => toast(`Could not tell whether ${machine} opens in an editor here: ${errorMessage(error)}`, "error"),
      (listener) => {
        window.addEventListener("focus", listener);
        return () => window.removeEventListener("focus", listener);
      },
    );
  }, [workspace.moved_to]);

  const ports = `${workspace.port_base}–${workspace.port_base + Math.max(0, workspace.port_count - 1)}`;

  return (
    <header className="shrink-0 border-b border-line px-4 pb-2 pt-2.5" data-tauri-drag-region>
      <div className="flex items-center gap-2.5">
        <ShowSidebarButton className="-ml-1.5" />
        <HistoryButtons />
        <StatusDot workspace={workspace} />
        <WorkspaceTitle workspace={workspace} />
        {workspace.state !== "ready" && (
          <Badge tone={status.tone === "danger" ? "danger" : status.tone === "working" ? "working" : "neutral"}>
            {(workspace.state === "creating" || workspace.state === "archiving") && <Spinner size={10} />}
            {status.label}
          </Badge>
        )}
        {workspace.state === "ready" && workspace.activity !== "idle" && (
          <Badge tone={status.tone === "attention" ? "attention" : status.tone === "danger" ? "danger" : "working"}>
            {status.label}
          </Badge>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-1.5">
          <LogButton session={session} />
          {workspace.state === "ready" && !workspace.moved_to && (
            <>
              <OpenButton workspace={workspace} repos={repos} />
              <AddRepoButton workspace={workspace} />
            </>
          )}
          {workspace.state === "ready" && remote && (
            <OpenButton workspace={workspace} repos={repos} remote={remote.machine} issue={remote.issue} />
          )}
          {/* Its pull requests are made there, through this daemon. */}
          {workspace.state === "ready" && (
            <>
              {workspace.explore && !workspace.moved_to && (
                <Button size="sm" onClick={() => setPromoteOpen(true)} data-testid="promote">
                  <GitBranch size={12} /> Continue as workspace
                </Button>
              )}
              {!workspace.explore && (
                <Button size="sm" onClick={() => setPrOpen(true)}>
                  <GitPullRequest size={12} /> Create PRs
                </Button>
              )}
            </>
          )}
          {(workspace.state === "ready" || workspace.state === "cleanup_failed") && (
            <Button size="sm" onClick={() => void archive()} data-testid="archive">
              <Archive size={12} /> Archive
            </Button>
          )}
          {(workspace.state === "archived" || workspace.state === "cleanup_failed") && !workspace.explore && (
            <Button size="sm" onClick={restore} busy={restoring} data-testid="restore">
              <ArchiveRestore size={12} /> Restore
            </Button>
          )}
        </div>
      </div>

      <div className="mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-[12px] text-muted">
        {workspace.explore ? (
          <ExploreBases workspace={workspace} />
        ) : (
          <CopyText value={workspace.branch} title="Copy branch name">
            <GitBranch size={12} className="text-faint" />
            <span className="font-mono text-[11.5px]">{workspace.branch}</span>
          </CopyText>
        )}
        <WorkspaceTickets workspace={workspace} />
        {workspace.state !== "archived" && workspace.port_count > 0 && (
          <span title="Port block (CHAUFFEUR_PORT)" className="text-faint">
            ports <span className="font-mono text-[11.5px] text-muted">{ports}</span>
          </span>
        )}
        {!workspace.explore && (
          <PrRow
            repos={repos}
            status={prs}
            refreshable={workspace.state === "ready" || workspace.state === "archived"}
            onMerge={workspace.state === "ready" ? setMerging : undefined}
          />
        )}
      </div>

      {workspace.state === "cleanup_failed" && workspace.last_error && (
        <div className="mt-2 flex items-start gap-2 rounded-md border border-danger/30 bg-danger/10 px-3 py-1.5 text-[12px] text-danger selectable">
          <AlertTriangle size={13} className="mt-0.5 shrink-0" />
          <span>
            Cleanup failed: {workspace.last_error}. Fix it and archive again, or archive anyway.
          </span>
        </div>
      )}

      <TransferBanner workspace={workspace} session={session} />

      <ArchiveDialog open={archiveOpen} onClose={() => setArchiveOpen(false)} workspace={workspace} refusal={archiveRefusal} />
      <PromoteDialog open={promoteOpen} onClose={() => setPromoteOpen(false)} workspace={workspace} />
      <CreatePrDialog open={prOpen} onClose={() => setPrOpen(false)} workspace={workspace} />
      <MergeDialog
        pr={merging}
        prs={prs.prs ?? []}
        onClose={() => setMerging(null)}
        workspace={workspace}
        onMerged={prs.refresh}
      />
    </header>
  );
}

/**
 * The workspace's Linear tickets. In a ready workspace: × on hover detaches one, + attaches one,
 * the refresh icon reads them all from Linear again; one Linear never described stands out.
 * Many tickets fold into a "⋯" whose hover lists them, so they do not fill the header.
 */
function WorkspaceTickets({ workspace }: { workspace: WorkspaceInfo }) {
  const editable = ticketsEditable(workspace);
  // What is being changed: a ticket's identifier, "add" or "refresh".
  const [busy, setBusy] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const change = async (tickets: string[], what: string, done: string) => {
    setBusy(what);
    try {
      const result = await client.call("workspace.set_tickets", { workspace: workspace.id, tickets });
      upsertWorkspace(result.workspace);
      if (result.warnings.length > 0) toast(result.warnings.join(" · "), "error");
      else toast(done);
      return true;
    } catch (error) {
      toast(`Could not change the tickets: ${errorMessage(error)}`, "error");
      return false;
    } finally {
      setBusy(null);
    }
  };
  const item = (ticket: TicketRef, maxWidth: string) => (
    <TicketItem
      key={ticket.identifier}
      ticket={ticket}
      maxWidth={maxWidth}
      busy={busy}
      onDetach={
        editable
          ? () => void change(withoutTicket(workspace.tickets, ticket.identifier), ticket.identifier, `${ticket.identifier} detached`)
          : undefined
      }
    />
  );
  return (
    <>
      {ticketsCollapsed(workspace.tickets) ? (
        <span className="group/tickets relative inline-flex" data-testid="tickets-collapsed">
          <button
            aria-label={`${workspace.tickets.length} tickets`}
            className="inline-flex h-5 items-center rounded px-1 text-faint hover:bg-hover hover:text-fg focus:bg-hover focus:text-fg"
          >
            <MoreHorizontal size={14} />
          </button>
          {/* pt-1.5 bridges the gap to the button, so the pointer can reach the list. */}
          <div className="absolute left-0 top-full z-50 hidden pt-1.5 group-focus-within/tickets:block group-hover/tickets:block">
            <div
              className="flex w-[460px] max-w-[calc(100vw-32px)] flex-col gap-1 rounded-xl border border-line-strong bg-sidebar p-2 shadow-2xl shadow-black/70"
              data-testid="tickets-list"
            >
              {workspace.tickets.map((ticket) => item(ticket, "max-w-full"))}
            </div>
          </div>
        </span>
      ) : (
        workspace.tickets.map((ticket) => item(ticket, "max-w-[300px]"))
      )}
      {editable && (
        <span className="inline-flex items-center gap-0.5">
          <button
            title="Attach a Linear ticket: the agents read it from their next message"
            onClick={() => setAdding(true)}
            disabled={busy != null}
            className="inline-flex items-center gap-0.5 rounded px-0.5 text-faint hover:text-fg disabled:opacity-50"
            data-testid="add-ticket"
          >
            <Plus size={11} /> Ticket
          </button>
          {workspace.tickets.length > 0 && (
            <IconButton
              title="Read the tickets from Linear again"
              disabled={busy != null}
              onClick={() =>
                void change(
                  workspace.tickets.map((t) => t.identifier),
                  "refresh",
                  "Tickets read again from Linear",
                )
              }
              className="h-5 w-5"
              data-testid="refresh-tickets"
            >
              <RefreshCw size={11} className={busy === "refresh" ? "animate-spin" : ""} />
            </IconButton>
          )}
        </span>
      )}
      {adding && (
        <AddTicketDialog
          tickets={workspace.tickets}
          explore={workspace.explore}
          busy={busy === "add"}
          onClose={() => setAdding(false)}
          onAdd={async (ticket) => {
            const added = await change(withTicket(workspace.tickets, ticket.identifier), "add", `${ticket.identifier} attached`);
            if (added) setAdding(false);
          }}
        />
      )}
    </>
  );
}

/** One ticket: opens in Linear; × detaches it when the workspace allows (`onDetach`). `busy` is what is being changed. */
function TicketItem({
  ticket,
  maxWidth,
  busy,
  onDetach,
}: {
  ticket: TicketRef;
  maxWidth: string;
  busy: string | null;
  onDetach?: () => void;
}) {
  const unread = ticketUnread(ticket);
  return (
    <span className={cx("group inline-flex items-center gap-0.5", maxWidth)} data-ticket={ticket.identifier}>
      <button
        title={ticketTooltip(ticket)}
        onClick={() => ticket.url && void openExternal(ticket.url)}
        className={cx("inline-flex min-w-0 items-center gap-1", ticket.url ? "hover:text-fg" : "cursor-default")}
        data-unread={unread || undefined}
      >
        {unread && <AlertTriangle size={11} className="shrink-0 text-attention" />}
        <span className={cx("shrink-0 whitespace-nowrap font-medium", unread ? "text-attention" : "text-fg/90")}>
          {ticket.identifier}
        </span>
        {ticket.title ? (
          <span className="truncate text-faint">{ticket.title}</span>
        ) : (
          <span className="shrink-0 italic text-faint">not read from Linear</span>
        )}
      </button>
      {onDetach && (
        <button
          title={`Detach ${ticket.identifier}`}
          aria-label={`Detach ${ticket.identifier}`}
          disabled={busy != null}
          onClick={onDetach}
          className={cx(
            "shrink-0 rounded p-px text-faint hover:bg-hover hover:text-fg focus:opacity-100 disabled:pointer-events-none",
            busy === ticket.identifier ? "opacity-100" : "opacity-0 group-hover:opacity-100",
          )}
          data-testid="detach-ticket"
        >
          {busy === ticket.identifier ? <Spinner size={10} /> : <XIcon size={10} />}
        </button>
      )}
    </span>
  );
}

/** Picks a ticket to attach: yours, any found in Linear, or an identifier typed. */
function AddTicketDialog({
  tickets,
  explore,
  busy,
  onClose,
  onAdd,
}: {
  tickets: TicketRef[];
  explore: boolean;
  busy: boolean;
  onClose: () => void;
  onAdd: (ticket: TicketRef) => void;
}) {
  const search = useTicketSearch(new Set(tickets.map((t) => t.identifier)), (ticket) => {
    if (!busy) onAdd(ticket);
  });
  const { setOpen } = search;
  useEffect(() => setOpen(true), [setOpen]);
  return (
    <Modal open onClose={onClose} title="Attach a ticket" width={560}>
      <div className="relative">
        <input
          autoFocus
          className={inputClass}
          placeholder="Pick one of your tickets, search Linear, or type an identifier…"
          value={search.query}
          onChange={(e) => search.setQuery(e.target.value)}
          onKeyDown={search.onKeyDown}
          disabled={busy}
          data-testid="ticket-input"
        />
        {(busy || search.loading) && <Spinner size={12} className="absolute right-2 top-2 text-faint" />}
      </div>
      <TicketResults search={search} className="mt-2 overflow-hidden rounded-md border border-line bg-bg" />
      <p className="mt-2 text-[11.5px] text-faint">
        {explore ? "" : "It moves to In Progress as at creation, if the Linear settings say so. "}
        The agents read it from their next message.
      </p>
    </Modal>
  );
}

/** An exploration's place of a branch: read-only, and each repo's base branch and commit. */
function ExploreBases({ workspace }: { workspace: WorkspaceInfo }) {
  const checkouts = workspace.worktrees.filter((w) => !w.removed && w.origin === "workspace");
  return (
    <>
      <span
        title="Read-only: the agents read and explain; they cannot edit, commit, push or open pull requests"
        className="inline-flex items-center gap-1 text-fg/90"
        data-testid="explore-badge"
      >
        <Telescope size={12} className="text-faint" /> Exploration
        <span className="inline-flex items-center gap-0.5 text-faint">
          · <Lock size={10} /> read-only
        </span>
      </span>
      {checkouts.map((w) => (
        <span key={w.id} title={`${w.repo} checked out at ${w.base_branch}${w.base_commit ? ` (${w.base_commit})` : ""}`} className="inline-flex items-center gap-1">
          <span className="text-muted">{w.repo}</span>
          <span className="font-mono text-[11.5px] text-faint">
            @{w.base_branch}
            {w.base_commit && ` ${w.base_commit.slice(0, 7)}`}
          </span>
        </span>
      ))}
    </>
  );
}

function CopyText({ value, title, children }: { value: string; title: string; children: React.ReactNode }) {
  return (
    <button
      title={title}
      className="group inline-flex items-center gap-1 hover:text-fg"
      onClick={() => {
        void navigator.clipboard?.writeText(value).then(() => toast(`Copied ${value}`));
      }}
    >
      {children}
      <Copy size={10} className="opacity-0 group-hover:opacity-60" />
    </button>
  );
}

/**
 * `pr.status` of the workspace: on open, when a turn ends (the agent may have committed or
 * opened a PR), on demand, and every minute while the workspace is ready.
 */
function usePrStatus(workspace: WorkspaceInfo, session: WorkspaceSession): PrStatus {
  const [state, setState] = useState<{ prs: PrInfo[] | null; error: string | null; loading: boolean }>({
    prs: null,
    error: null,
    loading: false,
  });
  const [nonce, setNonce] = useState(0);
  const turnsEnded = useWorkspaceSelector(session, (s) => s.turnsEnded);
  // An exploration has no branch, so no pull request to follow.
  const active = !workspace.explore && (workspace.state === "ready" || workspace.state === "archived");
  const poll = active && workspace.state === "ready";
  useEffect(() => {
    if (!active) {
      setState({ prs: null, error: null, loading: false });
      return;
    }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const load = async () => {
      setState((s) => ({ ...s, loading: true }));
      try {
        const result = await client.call("pr.status", { workspace: workspace.id });
        if (!cancelled) setState({ prs: result.prs, error: null, loading: false });
      } catch (error) {
        if (!cancelled) setState({ prs: null, error: errorMessage(error), loading: false });
        if (isNotImplemented(error)) return;
      }
      if (!cancelled && poll) timer = setTimeout(load, 60_000);
    };
    void load();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [workspace.id, active, poll, turnsEnded, nonce]);
  return { ...state, refresh: () => setNonce((n) => n + 1) };
}

/**
 * The workspace's name; a double click renames it (the folder and the branch keep theirs), and the
 * discreet button beside it has Claude name the work again from the conversation so far.
 */
function WorkspaceTitle({ workspace }: { workspace: WorkspaceInfo }) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [naming, setNaming] = useState(false);
  const regenerate = async () => {
    setNaming(true);
    try {
      upsertWorkspace(await client.call("workspace.retitle", { workspace: workspace.id }));
    } catch (error) {
      toast(`Could not name it again: ${errorMessage(error)}`, "error");
    } finally {
      setNaming(false);
    }
  };
  const save = async () => {
    setEditing(false);
    if (draft.trim() === workspaceName(workspace)) return;
    try {
      upsertWorkspace(await client.call("workspace.rename", { workspace: workspace.id, title: draft }));
    } catch (error) {
      toast(`Could not rename: ${errorMessage(error)}`, "error");
    }
  };
  if (editing) {
    return (
      <input
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onFocus={(e) => e.target.select()}
        onBlur={() => void save()}
        onKeyDown={(e) => {
          if (e.key === "Enter") void save();
          if (e.key === "Escape") setEditing(false);
        }}
        placeholder={workspace.slug}
        className="min-w-0 max-w-[480px] flex-1 rounded border border-accent/60 bg-bg px-1.5 text-[15px] font-semibold text-fg focus:outline-none"
        data-testid="workspace-title-input"
      />
    );
  }
  return (
    <div className="group/title flex min-w-0 items-center gap-1">
      <h1
        className="truncate text-[15px] font-semibold text-fg"
        title={workspace.title ? `${workspace.slug} · double-click to rename` : "Double-click to rename"}
        onDoubleClick={() => {
          setDraft(workspaceName(workspace));
          setEditing(true);
        }}
        data-testid="workspace-title"
      >
        {workspaceName(workspace)}
      </h1>
      {workspace.state === "ready" && (
        <IconButton
          title="Name it again from the conversation"
          onClick={() => void regenerate()}
          disabled={naming}
          className={cx(
            "h-5 w-5 shrink-0 focus-visible:opacity-100",
            naming ? "opacity-100 disabled:opacity-100" : "opacity-0 group-hover/title:opacity-100",
          )}
          data-testid="workspace-title-regenerate"
        >
          {naming ? <Spinner size={11} /> : <Sparkles size={11} />}
        </IconButton>
      )}
    </div>
  );
}

function LogButton({ session }: { session: WorkspaceSession }) {
  const log = useWorkspaceSelector(session, (s) => s.log);
  const [open, setOpen] = useState(false);
  const warnings = log.filter((entry) => entry.level === "warning" || entry.level === "error").length;
  return (
    <>
      <IconButton title="Workspace log" onClick={() => setOpen(true)} className="relative">
        <ScrollText size={13} />
        {warnings > 0 && <span className="absolute right-0.5 top-0.5 h-1.5 w-1.5 rounded-full bg-attention" />}
      </IconButton>
      <Modal open={open} onClose={() => setOpen(false)} title="Workspace log" width={680}>
        <LogList entries={log} empty="Nothing logged yet." />
      </Modal>
    </>
  );
}

/** While the workspace is being created: its worktrees, setup and warnings as they happen. */
/**
 * A workspace being created, in its conversation: what it is doing, without blocking the chat (the
 * agent starts once it is ready; messages sent meanwhile wait). Its warnings stay until dismissed.
 */
function SetupProgress({ workspace, session }: { workspace: WorkspaceInfo; session: WorkspaceSession }) {
  const log = useWorkspaceSelector(session, (s) => s.log);
  const [open, setOpen] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const creating = workspace.state === "creating";
  // Warnings are shown after the creation only when this view saw it happen.
  const [sawCreation, setSawCreation] = useState(creating);
  useEffect(() => {
    if (creating) setSawCreation(true);
  }, [creating]);
  const warnings = log.filter((entry) => entry.level === "warning");
  if (!creating && (!sawCreation || warnings.length === 0 || dismissed)) return null;
  const last = [...log].reverse().find((entry) => entry.level === "progress" || entry.level === "warning");
  return (
    <div className="mx-auto w-full max-w-[860px] px-5 pb-2" data-testid="setup-progress">
      <div className={cx("rounded-md border bg-raised/60 px-3 py-2", warnings.length > 0 ? "border-attention/40" : "border-line")}>
        <div className="flex items-center gap-2 text-[12px]">
          <button onClick={() => setOpen((v) => !v)} className="flex min-w-0 flex-1 items-center gap-2 text-left">
            {creating ? (
              <Spinner size={11} className="shrink-0 text-working" />
            ) : (
              <AlertTriangle size={12} className="shrink-0 text-attention" />
            )}
            <span className="shrink-0 text-fg">
              {creating
                ? "Setting up the workspace"
                : `Set up with ${warnings.length} warning${warnings.length > 1 ? "s" : ""}`}
            </span>
            {creating && <span className="min-w-0 truncate font-mono text-[11.5px] text-faint">{last?.message ?? "starting…"}</span>}
            <span className="ml-auto shrink-0 text-[11px] text-faint">{open ? "hide" : "details"}</span>
          </button>
          {!creating && (
            <IconButton title="Dismiss" onClick={() => setDismissed(true)}>
              <XIcon size={12} />
            </IconButton>
          )}
        </div>
        {creating && !open && (
          <div className="mt-0.5 text-[11.5px] text-faint">The agent starts once it is ready; you can write already, messages wait.</div>
        )}
        {open && (
          <div className="mt-2">
            <LogList entries={log} empty="Starting…" />
          </div>
        )}
      </div>
    </div>
  );
}

function LogList({ entries, empty }: { entries: LogEntry[]; empty: string }) {
  if (entries.length === 0) return <div className="text-[12px] text-faint">{empty}</div>;
  return (
    <div className="space-y-0.5 font-mono text-[11.5px]">
      {entries.map((entry) => (
        <div
          key={entry.key}
          className={cx(
            "flex gap-2",
            entry.level === "warning" && "text-attention",
            entry.level === "error" && "text-danger",
            entry.level === "state" && "text-fg",
            entry.level === "progress" && "text-muted",
          )}
        >
          <span className="shrink-0 text-faint">{new Date(entry.ts).toLocaleTimeString()}</span>
          <span className="selectable">{entry.message}</span>
        </div>
      ))}
    </div>
  );
}

function AddRepoButton({ workspace }: { workspace: WorkspaceInfo }) {
  const [open, setOpen] = useState(false);
  const repos = useApp((s) => s.repos);
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const existing = workspaceRepos(workspace);
  useEffect(() => {
    if (open && repos == null) void loadRepos();
  }, [open, repos]);
  const candidates = (repos ?? [])
    .filter((r) => !existing.includes(r.name))
    .filter((r) => !filter || r.key.toLowerCase().includes(filter.toLowerCase()));

  const add = async (repo: string) => {
    setBusy(repo);
    try {
      upsertWorkspace(await client.call("workspace.add_repo", { workspace: workspace.id, repo }));
      toast(`${repo} added to ${workspaceName(workspace)}`);
      setOpen(false);
    } catch (error) {
      toast(`Could not add ${repo}: ${errorMessage(error)}`, "error");
    } finally {
      setBusy(null);
    }
  };

  return (
    <>
      <Button size="sm" onClick={() => setOpen(true)}>
        <Plus size={12} /> Repo
      </Button>
      <Modal open={open} onClose={() => setOpen(false)} title="Add a repo" width={460}>
        <input
          autoFocus
          className={inputClass}
          placeholder="Filter repos…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && candidates[0]) void add(candidates[0].key);
          }}
        />
        <div className="mt-2 max-h-[300px] overflow-y-auto">
          {repos == null && <div className="py-2 text-[12px] text-faint">Loading repos…</div>}
          {repos != null && candidates.length === 0 && <div className="py-2 text-[12px] text-faint">No other repo.</div>}
          {candidates.map((repo) => (
            <button
              key={repo.key}
              disabled={busy != null}
              onClick={() => void add(repo.key)}
              className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left hover:bg-hover disabled:opacity-60"
            >
              <span className="shrink-0 text-fg">{repo.key}</span>
              <span className="truncate text-[11.5px] text-faint">{repo.description}</span>
              {busy === repo.key && <Spinner size={11} className="ml-auto" />}
            </button>
          ))}
        </div>
      </Modal>
    </>
  );
}
