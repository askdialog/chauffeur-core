// The models a picker offers: this machine's agents', or, inside a workspace that runs on another
// machine, that machine's (its logins, its agents' versions), read through this daemon.

import type { AgentModels, WorkspaceInfo } from "@chauffeur/protocol";
import { createContext, useCallback, useContext, useEffect, useState } from "react";
import { client, loadModels, useApp } from "./app";

/** Another machine's catalog: null while it is being read (again). */
export type Catalog = {
  models: AgentModels[] | null;
  loading: boolean;
  /** Asks that machine's agents for their models again. */
  refresh: () => void;
};

/** Undefined: none given here, the app's own catalog applies. */
export const CatalogContext = createContext<Catalog | undefined>(undefined);

export function useCatalog(): AgentModels[] | null {
  const given = useContext(CatalogContext);
  const own = useApp((s) => s.models);
  return given === undefined ? own : given.models;
}

/** Reads the catalog in use again: this machine's, or the other machine's inside a moved workspace. */
export function useCatalogLoader(): { loading: boolean; load: (refresh: boolean) => void } {
  const given = useContext(CatalogContext);
  const loading = useApp((s) => s.modelsLoading);
  if (given) return { loading: given.loading, load: (refresh) => refresh && given.refresh() };
  return { loading, load: (refresh) => void loadModels(refresh) };
}

/** Whether the catalog in use is another machine's: favorites it lacks are not offered there. */
export function useRemoteCatalog(): boolean {
  return useContext(CatalogContext) !== undefined;
}

/** Waits before asking a machine that did not answer again. */
const RETRY_MS = 5000;

/** The catalog of the machine `workspace` runs on, when it moved; undefined otherwise. */
export function useWorkspaceCatalog(workspace: Pick<WorkspaceInfo, "id" | "moved_to">): Catalog | undefined {
  const key = workspace.moved_to ? `${workspace.id}@${workspace.moved_to}` : null;
  const [read, setRead] = useState<{ key: string; models: AgentModels[] } | null>(null);
  const [loading, setLoading] = useState(false);
  // Bumped to ask again: a refresh, or a retry after a failure.
  const [asked, setAsked] = useState<{ count: number; refresh: boolean }>({ count: 0, refresh: false });
  useEffect(() => {
    if (!key) return;
    let disposed = false;
    let retry: ReturnType<typeof setTimeout> | null = null;
    setLoading(true);
    client
      .call("models.list", { refresh: asked.refresh, workspace: workspace.id })
      .then((result) => {
        if (!disposed) setRead({ key, models: result.agents });
      })
      // Not reachable yet: asked again a moment later (this machine's catalog would offer what
      // the other cannot run).
      .catch(() => {
        if (!disposed) retry = setTimeout(() => setAsked((a) => ({ count: a.count + 1, refresh: false })), RETRY_MS);
      })
      .finally(() => {
        if (!disposed) setLoading(false);
      });
    return () => {
      disposed = true;
      if (retry) clearTimeout(retry);
    };
  }, [key, workspace.id, asked]);
  const refresh = useCallback(() => setAsked((a) => ({ count: a.count + 1, refresh: true })), []);
  if (!key) return undefined;
  return { models: read?.key === key ? read.models : null, loading, refresh };
}
