import type { RepoInfo } from "@chauffeur/protocol";
import { Check, Folder, GitBranch, Loader2, Lock, Plus, RefreshCw, Search } from "lucide-react";
import { type ReactNode, type RefObject, useState } from "react";
import { relativeTime } from "@/lib/format";
import { openExternal } from "@/lib/native";
import {
  ALL_OWNERS,
  GITHUB_ACCESS_URL,
  ownerAvatar,
  shortAge,
  showsOwner,
} from "@/lib/repoPicker";
import { suggestedRepoName } from "@/lib/newRepo";
import { forgeName } from "@/lib/forge";
import { useForge } from "@/state/gitlab";
import { NewRepoForm } from "./NewRepoForm";
import { Button, cx, Kbd, Popover } from "./ui";
import { useRepoPicker } from "./useRepoPicker";

const WIDTH = 470;

/**
 * The creation dialog's repo picker, opened from its "Add repo" button: a search, a filter per
 * GitHub owner, then the chosen repos, Recent, each owner's repos and the clones only found on
 * this Mac. ↑↓ move the ring, ↵ adds or removes, ⌫ in an empty search removes the last; a pasted
 * GitHub URL is offered as a repo of its own, and a repo can be created on GitHub, named from the
 * search. Says when GitHub last listed the repos, and why it could not.
 */
export function RepoPopover({
  anchor,
  onClose,
  selection,
  onToggle,
  single = false,
  allowCreate = true,
}: {
  anchor: RefObject<HTMLElement | null>;
  onClose: () => void;
  /** Keys of the chosen repos (`name`, or `owner/name` outside the main organization). */
  selection: string[];
  onToggle: (repo: string) => void;
  single?: boolean;
  allowCreate?: boolean;
}) {
  const { filter, setFilter, owner, setOwner, view, navigation, pages, status, pick, onKeyDown, showScopeHint, catalogCount } = useRepoPicker(selection, onToggle);
  // GitHub unless the installation is set to a GitLab, where repos are listed, not created.
  const forge = useForge();
  const gitlab = forge === "gitlab";
  const forgeLabel = forgeName(forge);
  const { repos, query, offered, sections, options, owners, unknown } = view;
  const { listRef, active, onHover } = navigation;
  const { catalogError, githubError, refreshedAt } = status;
  /** The name the "New repository" view opens with; null while the list shows. */
  const [creating, setCreating] = useState<string | null>(null);
  const suggestion = suggestedRepoName(filter);
  const canCreate = allowCreate && !gitlab;
  const offerCreate = canCreate && !!suggestion && !offered && !repos.some((repo) => repo.name.toLowerCase() === suggestion.toLowerCase());

  if (creating !== null) {
    return (
      <Popover anchor={anchor} width={WIDTH} height={420} onClose={onClose} testId="repo-popover">
        <NewRepoForm
          initialName={creating}
          onCancel={() => setCreating(null)}
          onCreated={(repo) => {
            setCreating(null);
            pick(repo.key);
          }}
        />
      </Popover>
    );
  }

  return (
    <Popover anchor={anchor} width={WIDTH} height={420} onClose={onClose} testId="repo-popover">
      <div className="flex h-[34px] shrink-0 items-center gap-[7px] border-b border-line px-2.5">
        <Search size={13} className="shrink-0 text-faint" />
        <input
          data-autofocus
          className="h-full min-w-0 flex-1 bg-transparent text-[12.5px] text-fg placeholder:text-faint focus:outline-none"
          placeholder={gitlab ? "Filter projects…" : "Filter repos, or paste a GitHub URL…"}
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          onKeyDown={onKeyDown}
          aria-controls="repo-options"
          aria-activedescendant={options.length > 0 ? `repo-option-${active}` : undefined}
          data-testid="repo-filter"
        />
        <button type="button" title={`Refresh from ${forgeLabel}`} aria-label={`Refresh from ${forgeLabel}`} disabled={status.refreshDisabled} className="shrink-0 text-faint hover:text-fg disabled:opacity-40" onClick={pages.refresh}>
          <RefreshCw size={12} className={pages.loading ? "animate-spin" : ""} />
        </button>
        {canCreate && (
          <button type="button" title="New repository on GitHub" aria-label="New repository on GitHub" className="shrink-0 text-faint hover:text-fg" onClick={() => setCreating(suggestion)} data-testid="new-repo">
            <Plus size={13} />
          </button>
        )}
      </div>

      {githubError && (
        <div className="mx-2.5 mt-2 mb-1 flex shrink-0 items-start gap-[9px] rounded-[7px] border border-danger/30 bg-danger/10 px-[11px] py-[9px] text-danger" data-testid="repos-github-error">
          <span className="mt-px shrink-0">
            {gitlab ? <GitBranch size={13} /> : <GithubMark size={13} />}
          </span>
          <div className="min-w-0 flex-1">
            <div className="font-medium text-fg">{forgeLabel} didn't answer: some repos may be missing</div>
            <div className="truncate font-mono text-[11px]" title={githubError}>
              {githubError}
            </div>
          </div>
          <Button size="sm" className="shrink-0" onClick={pages.retry} busy={pages.loading}>
            {!pages.loading && <RefreshCw size={11} />} Retry
          </Button>
        </div>
      )}

      {owners.length > 0 && !offered && (
        <div
          className="flex shrink-0 items-center gap-0.5 overflow-x-auto border-b border-line px-1.5 py-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden"
          role="tablist"
          aria-label="Owner"
          onWheel={(e) => {
            // A mouse wheel only scrolls vertically: turn it into a horizontal scroll.
            if (Math.abs(e.deltaY) > Math.abs(e.deltaX)) e.currentTarget.scrollLeft += e.deltaY;
          }}
        >
          <OwnerFilter label="All" count={catalogCount} on={owner === ALL_OWNERS} onClick={() => setOwner(ALL_OWNERS)} />
          {owners.map((o) => (
            <OwnerFilter
              key={o.owner}
              label={o.owner}
              count={o.count}
              on={owner === o.owner}
              onClick={() => setOwner(o.owner)}
              icon={<OwnerAvatar owner={o.owner} org={repos.find((r) => r.owner === o.owner)?.owner_is_org ?? true} size={14} />}
            />
          ))}
        </div>
      )}

      <div ref={listRef} id="repo-options" className="min-h-0 flex-1 overflow-y-auto px-1 pt-0.5 pb-1" role="listbox" aria-multiselectable={!single}>
        {status.showSkeleton && (
          <div role="group" aria-label="Loading">
            <SectionTitle title="Repos" />
            {[[120, 150], [150, 110], [96, 170], [132, 90]].map(([name, meta], i) => (
              <div key={i} className="flex h-7 items-center gap-2 px-2">
                <span className="h-3.5 w-3.5 shrink-0 rounded-[3px] border border-line-strong" />
                <span className="h-2 animate-pulse rounded bg-line-strong/70" style={{ width: name }} />
                <span className="ml-auto h-2 animate-pulse rounded bg-line-strong/70" style={{ width: meta }} />
              </div>
            ))}
          </div>
        )}
        {catalogError && <div className="px-2 py-1.5 text-[12px] text-danger">Could not list repos: {catalogError}</div>}
        {offered && (
          <>
            <Row index={0} active={active === 0} onHover={onHover} onPick={() => pick(offered)} selected={selection.includes(offered)}>
              <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-fg text-bg">
                <GithubMark size={10} />
              </span>
              <span className="shrink-0 text-muted">Use</span>
              <span className="min-w-0 truncate text-fg">
                github.com/<b className="font-medium">{offered}</b>
              </span>
              <span className="ml-auto shrink-0"><Kbd>↵</Kbd></span>
            </Row>
            <div className="pb-1.5 pl-[34px] pr-2 pt-1 text-[11.5px] text-faint">Cloned on first use.</div>
          </>
        )}
        {status.showEmpty && (
          <div className="px-2 pt-3.5 pb-3 text-center">
            <div className="text-[12.5px] text-muted">{filter.trim() ? `No ${owner ? "" : "loaded "}repo matches “${filter.trim()}”.` : "No repo to pick yet."}</div>
            {gitlab ? (
              <div className="mt-0.5 text-[11.5px] text-faint">Check that your GitLab account is a member of its project or group.</div>
            ) : (
              <div className="mt-0.5 text-[11.5px] text-faint">
                Paste its GitHub URL, or{" "}
                <button type="button" className="text-muted underline hover:text-fg" onClick={() => void openExternal(GITHUB_ACCESS_URL)}>
                  check access ↗
                </button>{" "}
                if it lives in an SSO organization.
              </div>
            )}
          </div>
        )}
        {sections.map((section) => (
          <div key={section.id} role="group" aria-label={section.title}>
            <SectionTitle title={section.title} count={section.count ? `${section.count} loaded` : null} />
            {section.repos.map((repo, index) => {
              const i = section.start + index;
              return (
                <Row key={repo.key} index={i} active={active === i} onHover={onHover} onPick={() => pick(repo.key)} selected={selection.includes(repo.key)} repo={repo.key}>
                  <RepoAvatar repo={repo} />
                  <span className="shrink-0 whitespace-nowrap text-fg">
                    {!section.owner && showsOwner(repo, repos) && <span className="text-faint">{repo.owner}/</span>}
                    <Highlight text={repo.name} query={query} />
                  </span>
                  {repo.private && (
                    <span title="Private" className="-ml-[3px] shrink-0 text-faint">
                      <Lock size={11} />
                    </span>
                  )}
                  {repo.is_default && <span title="In the repos a new workspace starts with (Settings)" className="shrink-0 text-[10.5px] text-faint">default</span>}
                  {repo.local && (
                    <span
                      title={`Only on this Mac, no ${forgeLabel} remote`}
                      className="inline-flex h-4 shrink-0 items-center gap-[3px] rounded border border-line-strong px-[5px] text-[10.5px] text-muted"
                    >
                      <Folder size={10} />
                      local
                    </span>
                  )}
                  <span className="min-w-0 flex-1 truncate pl-1.5 text-right text-[11.5px] text-faint">
                    {repo.local
                      ? repo.clone_path?.replace(/^\/(?:Users|home)\/[^/]+/, "~")
                      : repo.description || (repo.clone_path ? "" : "cloned on first use")}
                  </span>
                  <span title={repo.pushed_at ? "Last push" : undefined} className="w-[30px] shrink-0 text-right text-[11px] tabular-nums text-faint">
                    {shortAge(repo.pushed_at)}
                  </span>
                </Row>
              );
            })}
          </div>
        ))}
        {unknown.map((key) => (
          <div key={key} className="flex items-center gap-2 px-2 py-1 text-[12px] text-muted">
            <GitBranch size={11} /> {key} <span className="text-faint">(from a ticket)</span>
          </div>
        ))}
        {offerCreate && (
          <button
            type="button"
            className="mt-0.5 flex h-7 w-full min-w-0 items-center gap-2 rounded-[5px] px-2 text-left text-[12.5px] text-muted hover:bg-hover hover:text-fg"
            onClick={() => setCreating(suggestion)}
            data-testid="create-repo-from-search"
          >
            <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-[4px] border border-dashed border-line-strong">
              <Plus size={10} />
            </span>
            <span className="min-w-0 truncate">
              Create <b className="font-medium text-fg">{suggestion}</b> on GitHub…
            </span>
          </button>
        )}
        {pages.nextCursor && (
          <div className="flex justify-center py-2">
            <Button size="sm" onClick={pages.loadMore} busy={pages.loading} disabled={!!pages.error} data-testid="repos-load-more">Load more repos</Button>
          </div>
        )}
      </div>

      {showScopeHint && (
        <div className="border-t border-line px-2.5 py-2 text-[11.5px] text-muted" data-testid="repos-search-scope">
          {gitlab
            ? "Showing loaded matches. Choose a group above to search all its projects on GitLab."
            : "Showing loaded matches. Choose an owner above to search all its repos on GitHub."}
        </div>
      )}
      {pages.truncated && (
        <div className="border-t border-line px-2.5 py-2 text-[11.5px] text-muted">{forgeLabel} limits search results. Use a more specific search to find other repos.</div>
      )}
      <div className="flex shrink-0 items-center gap-3 border-t border-line px-2.5 py-1.5 text-[11px] text-faint">
        {githubError ? null : status.loading ? (
          <span className="flex items-center gap-1.5">
            <Loader2 size={11} className="animate-spin" /> Listing your repos on {forgeLabel}…
          </span>
        ) : (
          <>
            <span className="flex items-center gap-1"><Kbd>↑↓</Kbd> move</span>
            <span className="flex items-center gap-1"><Kbd>↵</Kbd> add</span>
            <span className="flex items-center gap-1"><Kbd>⌫</Kbd> remove last</span>
          </>
        )}
        <span className={cx("truncate", !githubError && "ml-auto")}>
          {githubError ? (
            <span>Check {forgeLabel} access, then Retry.</span>
          ) : (
            refreshedAt && status.checked && (
              <span className="flex items-center gap-1" data-testid="repos-synced">
                <RefreshCw size={10} /> Checked {forgeLabel} {relativeTime(refreshedAt)}
              </span>
            )
          )}
        </span>
      </div>
    </Popover>
  );
}

function SectionTitle({ title, count }: { title: string; count?: string | null }) {
  return (
    <div className="flex items-baseline justify-between px-2 pt-2 pb-[3px] text-[10.5px] font-medium tracking-[.05em] text-faint uppercase">
      <span>{title}</span>
      {count && <span className="font-normal tracking-normal tabular-nums normal-case">{count}</span>}
    </div>
  );
}

function Row({
  index,
  active,
  selected,
  repo,
  onHover,
  onPick,
  children,
}: {
  index: number;
  active: boolean;
  selected: boolean;
  repo?: string;
  onHover: (index: number) => void;
  onPick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      role="option"
      id={`repo-option-${index}`}
      aria-selected={selected}
      data-option={index}
      data-repo={repo}
      tabIndex={-1}
      onMouseMove={() => !active && onHover(index)}
      onClick={onPick}
      className={cx(
        "flex h-7 w-full min-w-0 items-center gap-2 rounded-[5px] px-2 text-left text-[12.5px]",
        active && "bg-hover ring-1 ring-accent/55 ring-inset",
      )}
    >
      {repo != null && (
        <span
          className={cx(
            "flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-[3px] border",
            selected ? "border-accent bg-accent text-white" : "border-line-strong",
          )}
        >
          {selected && <Check size={10} strokeWidth={3} />}
        </span>
      )}
      {children}
    </button>
  );
}

function OwnerFilter({ label, count, on, onClick, icon }: { label: string; count: number; on: boolean; onClick: () => void; icon?: ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={on}
      title={`${count} repos loaded`}
      onClick={onClick}
      className={cx(
        "inline-flex h-[22px] shrink-0 items-center gap-[5px] rounded-[5px] px-[7px] text-[11.5px] whitespace-nowrap",
        on ? "bg-hover text-fg ring-1 ring-line-strong ring-inset" : "text-muted hover:text-fg",
      )}
      data-owner={label}
    >
      {icon}
      {label}
      <span className={cx("tabular-nums", on ? "text-muted" : "text-faint")}>{count}</span>
    </button>
  );
}

function RepoAvatar({ repo }: { repo: RepoInfo }) {
  if (repo.local || !repo.owner) {
    return (
      <span className="flex h-4 w-4 shrink-0 items-center justify-center rounded-[4px] border border-line-strong bg-raised text-muted">
        <Folder size={10} />
      </span>
    );
  }
  return <OwnerAvatar owner={repo.owner} org={repo.owner_is_org} size={16} />;
}

/** An owner's initials on a color of its own: square for an organization, round for a person,
 *  as GitHub draws them. */
function OwnerAvatar({ owner, org, size }: { owner: string; org: boolean; size: 14 | 16 }) {
  const { initials, color } = ownerAvatar(owner);
  return (
    <span
      aria-hidden
      className={cx(
        "flex shrink-0 items-center justify-center font-semibold tracking-[-.02em] text-white",
        org ? (size === 14 ? "rounded-[3.5px]" : "rounded-[4px]") : "rounded-full",
        size === 14 ? "h-3.5 w-3.5 text-[7px]" : "h-4 w-4 text-[8.5px]",
      )}
      style={{ background: color }}
    >
      {initials}
    </span>
  );
}

function Highlight({ text, query }: { text: string; query: string }) {
  const needle = query.trim().toLowerCase();
  const at = needle ? text.toLowerCase().indexOf(needle) : -1;
  if (at < 0) return <>{text}</>;
  return (
    <>
      {text.slice(0, at)}
      <mark className="bg-transparent font-semibold text-fg underline decoration-accent/70 underline-offset-2">{text.slice(at, at + needle.length)}</mark>
      {text.slice(at + needle.length)}
    </>
  );
}

/** GitHub's mark (lucide no longer ships brand icons). */
function GithubMark({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C6 2 5 2 5 2c-.3 1.15-.3 2.35 0 3.5A5.403 5.403 0 0 0 4 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4" />
      <path d="M9 18c-4.51 2-5-2-7-2" />
    </svg>
  );
}
