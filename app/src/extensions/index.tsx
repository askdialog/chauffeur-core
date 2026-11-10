// The views of the installed extensions. A view takes its extension's lease first, then loads
// the revision the daemon keeps for it, and only then renders the extension's component; it
// keeps the lease until it unmounts (a hidden but retained Database tab included).

import { useEffect, useState, type ComponentType, type ReactNode } from "react";
import type { ExtensionId, ExtensionInfo, WorkspaceInfo } from "@chauffeur/protocol";
import type { ViewId } from "@/lib/splitLayout";
import { EXTENSION_VIEWS, extensionEnabled } from "@/lib/extensions";
import { errorMessage } from "@/lib/rpc";
import { applyExtensions, client, openSettings, useApp } from "@/state/app";
import { ErrorBoundary } from "@/components/ErrorBoundary";
import { Button, EmptyState, Spinner } from "@/components/ui";
import { holdLease, loadExtensionUi, retryLeases, useExtensionUi, useLease, type ExtensionContribution } from "./registry";

type WorkspaceContribution = Extract<(typeof EXTENSION_VIEWS)[number], { placement: "workspace" }>;
export const WORKSPACE_EXTENSION_VIEWS = EXTENSION_VIEWS.filter((view): view is WorkspaceContribution => view.placement === "workspace");

const LABELS: Record<ExtensionId, string> = { database: "Database", office: "Office" };
const NO_UI_RETRY_MS = 1000;

/** Why a package cannot be shown yet, from the catalogue; null when it should be ready. */
export function packageWaitReason(extension: ExtensionInfo | undefined): { title: string; detail: string | null; busy: boolean } | null {
  const label = extension?.name ?? "This extension";
  const pkg = extension?.package;
  if (!extension || !pkg) return { title: `Loading ${label}…`, detail: null, busy: true };
  switch (pkg.phase) {
    case "installed":
    case "waiting":
      return null;
    case "starting":
      return { title: `Starting ${label}…`, detail: null, busy: true };
    case "downloading":
      return { title: `Installing ${label}…`, detail: "Chauffeur is downloading and checking the extension.", busy: true };
    case "missing":
      return { title: `${label} is not installed`, detail: pkg.error ?? "Install it from Settings › Extensions.", busy: false };
    case "failed":
      return { title: `${label} could not start`, detail: pkg.error ?? "Restore or reinstall it from Settings › Extensions.", busy: false };
  }
}

/**
 * Holds the extension's lease while mounted, and gives the contribution of the revision the
 * daemon keeps for this lease, once loaded; else what to show meanwhile.
 */
function useLeasedContribution(id: ExtensionId): { contribution: ExtensionContribution; revision: string } | { waiting: ReactNode } {
  const extension = useApp((state) => state.extensions?.find((entry) => entry.id === id));
  const lease = useLease(id);
  const ui = useExtensionUi(id);
  const [target, setTarget] = useState<{ generation: number; revision: string | null; error: string | null } | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => holdLease(id), [id]);
  // The package became ready: ask for the lease now rather than at the next retry.
  const phase = extension?.package?.phase;
  useEffect(() => {
    if (phase === "installed" || phase === "waiting") retryLeases();
  }, [phase]);

  useEffect(() => {
    if (!lease.state.held) return;
    let cancelled = false;
    let retry: ReturnType<typeof setTimeout> | null = null;
    const generation = lease.generation;
    void (async () => {
      try {
        // Asked once the lease is held: the revision it pins, not one the catalogue had before.
        const extensions = await client.call("extensions.list", {});
        applyExtensions(extensions);
        const metadata = extensions.find((entry) => entry.id === id)?.package?.ui;
        // Not there yet (a first install finishing): still loading, asked again shortly.
        if (!metadata) {
          if (!cancelled) retry = setTimeout(() => setAttempt((n) => n + 1), NO_UI_RETRY_MS);
          return;
        }
        await loadExtensionUi(id, metadata);
        if (!cancelled) setTarget({ generation, revision: metadata.revision, error: null });
      } catch (error) {
        if (!cancelled) setTarget({ generation, revision: null, error: errorMessage(error) });
      }
    })();
    return () => {
      cancelled = true;
      if (retry) clearTimeout(retry);
    };
  }, [id, lease.state.held, lease.generation, attempt]);

  if (lease.state.held && target?.generation === lease.generation && target.revision && ui.loaded?.revision === target.revision) {
    return { contribution: ui.loaded.contribution, revision: target.revision };
  }
  if (lease.state.held && target?.generation === lease.generation && target.error) {
    return {
      waiting: (
        <EmptyState title={`Could not open ${LABELS[id]}`}>
          <div className="selectable">{target.error}</div>
          <div className="mt-3 flex justify-center gap-2">
            <Button size="sm" onClick={() => setAttempt((n) => n + 1)}>Try again</Button>
            <Button size="sm" variant="secondary" onClick={() => openSettings(true, "extensions")}>Extensions</Button>
          </div>
        </EmptyState>
      ),
    };
  }
  const reason = lease.state.held ? null : packageWaitReason(extension);
  if (reason && !reason.busy) {
    return {
      waiting: (
        <EmptyState title={reason.title}>
          {reason.detail && <div className="selectable">{reason.detail}</div>}
          <div className="mt-3">
            <Button size="sm" variant="secondary" onClick={() => openSettings(true, "extensions")}>Extensions</Button>
          </div>
        </EmptyState>
      ),
    };
  }
  return {
    waiting: (
      <EmptyState icon={<Spinner />} title={reason?.title ?? `Loading ${LABELS[id]}…`}>
        {reason?.detail}
      </EmptyState>
    ),
  };
}

function LeasedView<P extends object>({ id, pick, props }: { id: ExtensionId; pick: (contribution: ExtensionContribution) => ComponentType<P> | undefined; props: P }) {
  const result = useLeasedContribution(id);
  if ("waiting" in result) return <>{result.waiting}</>;
  const Component = pick(result.contribution);
  if (!Component) return <EmptyState title={`${LABELS[id]} has no view here`} />;
  // Another revision is another component: it mounts afresh rather than reusing the old state.
  return (
    <ErrorBoundary key={result.revision}>
      <Component {...props} />
    </ErrorBoundary>
  );
}

const pickWorkspace = (contribution: ExtensionContribution) => contribution.workspace;
const pickGlobal = (contribution: ExtensionContribution) => contribution.global;

export function WorkspaceExtensionViews({ extensions, workspace, main, side, slot }: {
  extensions: ExtensionInfo[] | null;
  workspace: WorkspaceInfo;
  main: ViewId;
  side: ViewId | null;
  slot: (id: ViewId, content: ReactNode) => ReactNode;
}) {
  const [mounted, setMounted] = useState<Set<string>>(() => new Set([main, ...(side ? [side] : [])]));
  useEffect(() => {
    setMounted((previous) => new Set([...previous, main, ...(side ? [side] : [])]));
  }, [main, side]);
  return WORKSPACE_EXTENSION_VIEWS.map((view) => {
    if (!extensionEnabled(extensions, view.extension) || !mounted.has(view.id)) return null;
    return <div key={view.id} className="contents">{slot(view.id,
      <LeasedView id={view.extension} pick={pickWorkspace} props={{ workspace }} />,
    )}</div>;
  });
}

export function GlobalExtensionView({ id }: { id: Extract<(typeof EXTENSION_VIEWS)[number], { placement: "global" }>["id"] }) {
  const extensions = useApp((state) => state.extensions);
  if (!extensionEnabled(extensions, id)) return null;
  return <LeasedView id={id} pick={pickGlobal} props={{}} />;
}

/** Names of the loaded Office UI for the quick switcher, while its view is open. */
export function useExtensionMembers(id: ExtensionId): readonly string[] {
  return useExtensionUi(id).loaded?.contribution.members ?? [];
}
