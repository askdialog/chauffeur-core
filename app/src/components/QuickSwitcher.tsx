import type { SearchHit, SearchResult } from "@chauffeur/protocol";
import { FileText, Folder, GitPullRequest, MapPin, MessageSquare, Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { filterOptions, NO_CHOICES, type SearchChoices, searchExcerpt, searchFilters, semanticLine, workspaceKept } from "@/lib/search";
import { errorMessage } from "@/lib/rpc";
import { relativeTime, workspaceName, workspaceRepoNames } from "@/lib/format";
import { useExtensionMembers } from "@/extensions";
import { client, goToMember, goToWorkspace, openFile, openSearchMatch, openSettings, openSwitcher, orderedWorkspaces, toast, useApp } from "@/state/app";
import { Button, cx, StatusDot } from "./ui";

/** While search by meaning starts, the search is asked again this often, this many times. */
const STARTING_RETRY_MS = 1200;
const STARTING_RETRIES = 15;

/**
 * ⌘K: jump to a workspace by slug, ticket, branch or repo, and search every workspace's titles,
 * messages, notes and pull requests (by keywords and by meaning); in the Office, also go to a
 * member.
 */
export function QuickSwitcher() {
  const open = useApp((s) => s.switcherOpen);
  return open ? <SwitcherPanel /> : null;
}

function SwitcherPanel() {
  const workspaces = useApp((s) => s.workspaces);
  const sections = useApp((s) => s.sections);
  const archivedOnly = useApp((s) => s.switcherArchived);
  const [query, setQuery] = useState("");
  const [choices, setChoices] = useState<SearchChoices>(archivedOnly ? { ...NO_CHOICES, scope: "archived" } : NO_CHOICES);
  const [index, setIndex] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const [found, setFound] = useState<{ key: string; result: SearchResult | null; error: string | null } | null>(null);
  const [retry, setRetry] = useState(0);
  const search = query.trim();
  const searching = search.length > 0;
  const key = JSON.stringify([search, choices]);
  const result = searching && found?.key === key ? found.result : null;
  const loading = searching && found?.key !== key;
  const error = searching && found?.key === key ? found.error : null;
  useEffect(() => setRetry(0), [key]);
  useEffect(() => {
    if (!searching) return;
    let cancelled = false;
    let again: ReturnType<typeof setTimeout> | undefined;
    const timer = setTimeout(() => {
      client.call("search.query", { query: search, filters: searchFilters(choices, new Date()) }).then(
        (result) => {
          if (cancelled) return;
          setFound({ key, result, error: null });
          // Search by meaning is starting: ask again, its results come once it runs.
          if (result.status.semantic === "starting" && retry < STARTING_RETRIES) again = setTimeout(() => setRetry((n) => n + 1), STARTING_RETRY_MS);
        },
        (error) => { if (!cancelled) setFound({ key, result: null, error: errorMessage(error) }); },
      );
    }, 180);
    return () => { cancelled = true; clearTimeout(timer); clearTimeout(again); };
    // `key` stands for the query and the choices.
  }, [searching, key, retry]);

  const all = useMemo(
    () =>
      (archivedOnly
        ? orderedWorkspaces(workspaces, true)
        : [...orderedWorkspaces(workspaces, false, sections), ...orderedWorkspaces(workspaces, true)]
      ).map((e) => e.workspace),
    [workspaces, sections, archivedOnly],
  );
  const options = useMemo(() => filterOptions(Object.values(workspaces)), [workspaces]);
  const results = useMemo(() => {
    const q = query.trim().toLowerCase();
    const kept = all.filter((w) => workspaceKept(w, choices));
    if (!q) return kept.slice(0, 50);
    return kept
      .map((w) => {
        const haystack = [w.slug, w.title ?? "", w.branch, ...w.tickets.map((t) => `${t.identifier} ${t.title ?? ""}`), ...workspaceRepoNames(w)]
          .join(" ")
          .toLowerCase();
        const score = w.slug.toLowerCase().startsWith(q) ? 0 : haystack.includes(q) ? 1 : fuzzy(haystack, q) ? 2 : 3;
        return { w, score };
      })
      .filter((r) => r.score < 3)
      .sort((a, b) => choices.scope === "archived" ? 0 : a.score - b.score)
      .map((r) => r.w)
      .slice(0, 50);
  }, [all, query, choices]);
  // A workspace already listed by name is not listed again for its title.
  const hits = useMemo(() => {
    const listed = new Set(results.map((w) => w.id));
    return (result?.hits ?? []).filter((hit) => !(hit.kind === "workspace" && listed.has(hit.workspace_id)));
  }, [result, results]);

  // In the Office, the members' quarters first: "go to Carol", or only "Carol".
  const inOffice = useApp((s) => s.officeShown);
  const waiting = useApp((s) => s.officeWaiting);
  const roster = useExtensionMembers("office");
  const members = useMemo(() => {
    if (!inOffice || archivedOnly) return [];
    const q = query.trim().toLowerCase().replace(/^(go( to)?|goto)\s*/, "");
    return roster.filter((name) => name.toLowerCase().startsWith(q));
  }, [inOffice, archivedOnly, query, roster]);
  const rows = members.length + results.length + hits.length;
  const pickHit = (hit: SearchHit) => {
    if (hit.kind === "message" && hit.chat_id && hit.seq !== null) {
      const word = searchExcerpt(hit.snippet).find((part) => part.highlight)?.text ?? "";
      openSearchMatch(hit.workspace_id, hit.chat_id, hit.seq, word);
    } else {
      goToWorkspace(hit.workspace_id);
      if (hit.kind === "note" && hit.path) openFile(hit.workspace_id, hit.path);
    }
  };
  const pick = (i: number) => {
    if (i < members.length) goToMember(members[i]!);
    else if (results[i - members.length]) goToWorkspace(results[i - members.length]!.id);
    else {
      const hit = hits[i - members.length - results.length];
      if (hit) pickHit(hit);
    }
  };
  const line = result ? semanticLine(result.status) : null;
  const download = async () => {
    if (!result) return;
    try {
      await client.call("ollama.pull", { model: result.status.model });
      toast(`Downloading ${result.status.model}: search by meaning starts once it is there (Settings › Search shows its progress).`);
    } catch (err) {
      toast(`Could not download ${result.status.model}: ${errorMessage(err)}`, "error");
    }
  };

  useEffect(() => input.current?.focus(), []);
  useEffect(() => setIndex(0), [query, choices]);
  useEffect(() => setIndex((i) => Math.max(0, Math.min(i, rows - 1))), [rows]);
  useEffect(() => {
    document.querySelector('[data-switcher-selected="true"]')?.scrollIntoView({ block: "nearest" });
  }, [index]);
  const choose = <K extends keyof SearchChoices>(name: K, value: SearchChoices[K]) => setChoices((c) => ({ ...c, [name]: value }));

  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-black/50 pt-[14vh]" onMouseDown={() => openSwitcher(false)}>
      <div
        role="dialog"
        aria-label="Switch workspace"
        className="w-[680px] max-w-[calc(100vw-32px)] overflow-hidden rounded-lg border border-line-strong bg-panel shadow-2xl"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex items-center gap-2 border-b border-line px-3">
          <Search size={14} className="text-faint" />
          <input
            ref={input}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") openSwitcher(false);
              else if (e.key === "ArrowDown") {
                e.preventDefault();
                setIndex((i) => Math.min(i + 1, rows - 1));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setIndex((i) => Math.max(i - 1, 0));
              } else if (e.key === "Enter") pick(index);
            }}
            placeholder={archivedOnly ? "Search archived workspaces and conversations…" : inOffice ? "Go to someone, or to a workspace's house…" : "Switch to a workspace, or search everything…"}
            className="h-10 flex-1 bg-transparent text-[13px] text-fg placeholder:text-faint focus:outline-none"
          />
        </div>
        {(searching || archivedOnly) && (
          <div className="flex flex-wrap gap-1.5 border-b border-line px-3 py-1.5" data-testid="search-filters">
            <Choice label="Workspaces" value={choices.scope} onChange={(v) => choose("scope", v)} options={[["all", "All workspaces"], ["active", "Active"], ["archived", "Archived"]]} />
            <Choice label="Where" value={choices.where} onChange={(v) => choose("where", v)} options={[["all", "Everywhere"], ["titles", "Titles"], ["messages", "Messages"], ["notes", "Notes"]]} />
            <Choice label="Repo" value={choices.repo} onChange={(v) => choose("repo", v)} options={[["", "Any repo"], ...options.repos.map((r): [string, string] => [r, r])]} />
            {options.tickets.length > 0 && <Choice label="Ticket" value={choices.ticket} onChange={(v) => choose("ticket", v)} options={[["", "Any ticket"], ...options.tickets.map((t): [string, string] => [t, t])]} />}
            <Choice label="Date" value={choices.date} onChange={(v) => choose("date", v)} options={[["any", "Any date"], ["week", "Past week"], ["month", "Past month"], ["year", "Past year"]]} />
            <Choice label="Pull request" value={choices.pr} onChange={(v) => choose("pr", v)} options={[["any", "With or without PR"], ["with", "With a PR"], ["merged", "PR merged"]]} />
          </div>
        )}
        <div className="max-h-[400px] overflow-y-auto py-1">
          {members.map((name, i) => (
            <button
              key={name}
              data-switcher-selected={i === index}
              onMouseEnter={() => setIndex(i)}
              onClick={() => goToMember(name)}
              className={cx("flex w-full items-center gap-2 px-3 py-1.5 text-left", i === index && "bg-hover")}
              data-testid={`switcher-go-${name}`}
            >
              <MapPin size={13} className="text-faint" />
              <span className="text-fg">Go to {name}</span>
              <span className="text-[11.5px] text-faint">their quarter of the village</span>
              {(waiting[name] ?? 0) > 0 && (
                <span className="ml-auto rounded-full bg-attention px-1.5 text-[11px] font-bold text-black" title="Agents waiting for them">
                  {waiting[name]} ?
                </span>
              )}
            </button>
          ))}
          {rows === 0 && !searching && <div className="px-3 py-2 text-[12px] text-faint">{archivedOnly ? "No archived workspace matches." : "No workspace matches."}</div>}
          {searching && results.length > 0 && <div className="px-3 pb-1 pt-2 text-[11px] text-faint">Workspaces</div>}
          {results.map((w, i) => (
            <button
              key={w.id}
              data-switcher-selected={members.length + i === index}
              onMouseEnter={() => setIndex(members.length + i)}
              onClick={() => goToWorkspace(w.id)}
              className={cx("flex w-full items-center gap-2 px-3 py-1.5 text-left", members.length + i === index && "bg-hover")}
            >
              <StatusDot workspace={w} />
              <span className={cx("text-fg", (w.state === "archived" || w.state === "archiving") && "text-faint")}>{workspaceName(w)}</span>
              <span className="truncate text-[11.5px] text-faint">{workspaceRepoNames(w).join(" · ")}</span>
              {(w.state === "archived" || w.state === "archiving") && <span className="ml-auto text-[11px] text-faint">{w.state}</span>}
              {inOffice && w.state !== "archived" && w.state !== "archiving" && <span className="ml-auto shrink-0 text-[11px] text-faint">go to its house</span>}
            </button>
          ))}
          {searching && (
            <>
              <div className="px-3 pb-1 pt-2 text-[11px] text-faint">Found in workspaces{hits.length === 50 ? " · the first 50" : ""}</div>
              {loading && <div role="status" className="px-3 py-2 text-[12px] text-faint">Searching…</div>}
              {error && <div role="alert" className="px-3 py-2 text-[12px] text-faint">Could not search: {error}</div>}
              {!loading && !error && hits.length === 0 && <div className="px-3 py-2 text-[12px] text-faint">{results.length > 0 ? "Nothing else matches." : "Nothing matches."}</div>}
              {hits.map((hit, i) => (
                <HitRow
                  key={`${hit.kind}:${hit.workspace_id}:${hit.seq ?? hit.path ?? hit.url ?? ""}`}
                  hit={hit}
                  workspace={workspaces[hit.workspace_id] ? workspaceName(workspaces[hit.workspace_id]!) : hit.workspace_id}
                  selected={members.length + results.length + i === index}
                  onHover={() => setIndex(members.length + results.length + i)}
                  onPick={() => pickHit(hit)}
                />
              ))}
            </>
          )}
        </div>
        {searching && line && (
          <div className="flex items-center gap-2 border-t border-line px-3 py-1.5 text-[11.5px] text-faint" data-testid="semantic-status">
            <span className="flex-1">{line.text}</span>
            {line.action === "download" && <Button size="sm" onClick={() => void download()}>Download</Button>}
            {line.action === "settings" && <Button size="sm" onClick={() => openSettings(true, "search")}>Settings</Button>}
          </div>
        )}
      </div>
    </div>
  );
}

const KIND_ICONS = { message: MessageSquare, note: FileText, pull_request: GitPullRequest, workspace: Folder } as const;

function HitRow({ hit, workspace, selected, onHover, onPick }: { hit: SearchHit; workspace: string; selected: boolean; onHover: () => void; onPick: () => void }) {
  const Icon = KIND_ICONS[hit.kind];
  const where = hit.kind === "message" ? hit.chat_title ?? "Chat" : hit.kind === "note" ? hit.path : hit.kind === "pull_request" ? hit.title : null;
  return (
    <button
      data-testid="search-hit"
      data-switcher-selected={selected}
      onMouseEnter={onHover}
      onClick={onPick}
      className={cx("block w-full px-3 py-2 text-left", selected && "bg-hover")}
    >
      <div className="flex items-center gap-1.5 text-[12px]">
        <Icon size={12} className="shrink-0 text-faint" />
        <span className="truncate text-fg">{workspace}</span>
        {where && <span className="truncate text-faint">· {where}</span>}
        {hit.matched === "meaning" && <span className="shrink-0 rounded bg-hover px-1 text-[10.5px] text-faint" title="Found by meaning, not by these words">by meaning</span>}
        <span className="ml-auto shrink-0 text-[11px] text-faint">{relativeTime(hit.at)}</span>
      </div>
      <div className="mt-1 line-clamp-2 whitespace-pre-wrap break-words text-[12px] text-faint">
        {searchExcerpt(hit.snippet).map((part, n) => part.highlight ? <mark key={n} className="rounded-sm bg-attention/20 text-fg">{part.text}</mark> : part.text)}
      </div>
    </button>
  );
}

function Choice<V extends string>({ label, value, options, onChange }: { label: string; value: V; options: [V, string][]; onChange: (value: V) => void }) {
  const narrowed = value !== options[0]?.[0];
  return (
    <select
      aria-label={label}
      value={value}
      onChange={(e) => onChange(options.find(([id]) => id === e.target.value)?.[0] ?? value)}
      className={cx("h-6 rounded border bg-panel px-1.5 text-[11.5px] focus:outline-none", narrowed ? "border-accent/60 text-fg" : "border-line text-muted")}
    >
      {options.map(([id, name]) => <option key={id} value={id}>{name}</option>)}
    </select>
  );
}

function fuzzy(haystack: string, needle: string): boolean {
  let i = 0;
  for (const char of haystack) {
    if (char === needle[i]) i++;
    if (i === needle.length) return true;
  }
  return false;
}
