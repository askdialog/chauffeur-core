import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { ALL_OWNERS, repoPickerSearch, repoPickerView } from "@/lib/repoPicker";
import { useGithubSource } from "@/state/github";
import { useForge } from "@/state/gitlab";
import { useRepoCatalog } from "@/state/repos";
import { useRepoPages } from "./useRepoPages";

export function useRepoPicker(selection: string[], onToggle: (repo: string) => void) {
  const catalog = useRepoCatalog();
  // On a GitLab installation, GitHub's account and organizations are no owners of its projects.
  const gitlab = useForge() === "gitlab";
  const githubStatus = useGithubSource();
  const github = gitlab ? null : githubStatus;
  const account = github?.account ?? null;
  const [filter, setFilter] = useState("");
  const [owner, setOwner] = useState(ALL_OWNERS);
  const [highlight, setHighlight] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  // The repos listed first: the selection when the picker opened, taken again when the filter or
  // the owner changes. A click toggles a row's box in place; were the chosen repos moved to the
  // top at once, the rows would shift under the pointer and the next click would hit another repo.
  const [pinned, setPinned] = useState(selection);
  const latestSelection = useRef(selection);
  latestSelection.current = selection;
  // Scrolled into view only when the keyboard moves the ring: on hover, a row half hidden at the
  // list's edge would scroll the list under the pointer between mousedown and mouseup.
  const scrollToActive = useRef(false);
  const search = repoPickerSearch(owner, filter);
  const pages = useRepoPages({
    owner,
    query: search.remoteQuery,
    enabled: search.remoteEnabled,
    source: `${account}/${github?.organization ?? ""}`,
    revision: catalog.revision,
  });
  const knownOwners = [github?.organization ?? "", ...(catalog.sources ? [account ?? "", ...(github?.organizations ?? [])] : [])];
  const view = repoPickerView({ repos: catalog.repos ?? [], pages: pages.repos, selected: selection, pinned, search, account, knownOwners });
  const active = Math.min(highlight, Math.max(0, view.options.length - 1));
  useEffect(() => {
    setHighlight(0);
    setPinned(latestSelection.current);
  }, [filter, owner]);
  useEffect(() => {
    if (!scrollToActive.current) return;
    scrollToActive.current = false;
    listRef.current?.querySelector(`[data-option="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const pick = (key: string) => {
    onToggle(key);
    if (filter) setFilter("");
  };
  /** A repo just created: picked and listed first, the list being redrawn anyway. */
  const pickCreated = (key: string) => {
    onToggle(key);
    setPinned((keys) => (keys.includes(key) ? keys : [...keys, key]));
    if (filter) setFilter("");
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.metaKey || event.ctrlKey) return;
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      scrollToActive.current = true;
      setHighlight(Math.max(0, Math.min(view.options.length - 1, active + step)));
    } else if (event.key === "Enter" && view.options.length > 0) {
      event.preventDefault();
      pick(view.options[active]);
    } else if (event.key === "Backspace" && !filter && selection.length > 0) {
      onToggle(selection[selection.length - 1]);
    }
  };
  const githubError = pages.error ?? (pages.refreshedAt ? null : catalog.sync.githubError);
  const status = {
    catalogError: catalog.error,
    githubError,
    refreshedAt: pages.refreshedAt ?? catalog.sync.refreshedAt,
    showSkeleton: view.repos.length === 0 && (catalog.repos === null || pages.loading) && !catalog.error,
    showEmpty: catalog.repos !== null && view.options.length === 0 && !pages.loading && !githubError,
    loading: pages.loading || (catalog.repos === null && !catalog.error),
    checked: catalog.repos !== null,
    refreshDisabled: pages.loading || !search.remoteEnabled,
  };
  return {
    filter, setFilter, owner, setOwner, view, pages, status, pick, pickCreated, onKeyDown,
    showScopeHint: search.showScopeHint,
    catalogCount: catalog.repos?.length ?? 0,
    navigation: { listRef, active, onHover: setHighlight },
  };
}
