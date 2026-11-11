import type { OutgoingShare } from "@chauffeur/protocol";
import { Copy, ExternalLink, Link, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { client, toast } from "@/state/app";
import { openExternal } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { Button, EmptyState } from "./ui";
import { ShowSidebarButton } from "./ShowSidebarButton";

export function SharingPage() {
  const [shares, setShares] = useState<OutgoingShare[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [stopping, setStopping] = useState<string | null>(null);
  const generation = useRef(0);
  const stopPending = useRef(false);
  const refresh = useCallback(async () => {
    if (stopPending.current) return;
    const request = ++generation.current;
    setLoading(true);
    try {
      const result = await client.call("shares.list", {});
      if (request !== generation.current) return;
      setShares(result);
      setError(null);
    } catch (error) {
      if (request === generation.current) setError(errorMessage(error));
    } finally {
      if (request === generation.current) setLoading(false);
    }
  }, []);
  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 15000);
    return () => { window.clearInterval(timer); generation.current++; };
  }, [refresh]);

  async function stop(share: OutgoingShare) {
    stopPending.current = true;
    generation.current++;
    setLoading(false);
    setStopping(share.id);
    try {
      const source = share.source;
      if (source.kind === "design") await client.call("design.share_stop", { chat: source.chat });
      else if (source.kind === "workspace") await client.call("workspace.share_stop", { workspace: share.workspace });
      else await client.call("files.share_stop", { workspace: share.workspace, path: source.path });
      setShares((rows) => rows?.filter((row) => row.id !== share.id) ?? null);
      toast(`Stopped sharing ${share.name}`);
    } catch (error) {
      toast(`Could not stop sharing ${share.name}: ${errorMessage(error)}`, "error");
    } finally {
      stopPending.current = false;
      setStopping(null);
    }
  }

  async function open(share: OutgoingShare) {
    try {
      await openExternal(share.url);
    } catch (error) {
      toast(`Could not open the link: ${errorMessage(error)}`, "error");
    }
  }

  async function copy(share: OutgoingShare) {
    try {
      await navigator.clipboard.writeText(share.url);
      toast("Link copied");
    } catch (error) {
      toast(`Could not copy the link: ${errorMessage(error)}`, "error");
    }
  }

  return <section className="flex min-h-0 flex-1 flex-col" data-testid="sharing-page">
    <header className="flex items-center gap-3 border-b border-line px-6 py-4">
      <ShowSidebarButton />
      <Link size={18} className="text-muted" />
      <div className="flex-1"><h1 className="text-base font-medium">Sharing</h1>
        <p className="mt-1 text-xs text-muted">Your active links. Archiving a design keeps its link active; deleting it stops sharing.</p>
      </div>
      <Button onClick={() => void refresh()} disabled={loading}><RefreshCw size={13} /> Refresh</Button>
    </header>
    {error && <div role="alert" className="m-6 rounded-md border border-line p-4 text-sm">Could not load your shares: {error} <Button onClick={() => void refresh()}>Retry</Button></div>}
    {!shares && !error ? <EmptyState title="Loading shares…" /> : shares?.length === 0 ?
      <EmptyState title="Nothing shared yet">Share a design, workspace or file to see its link here.</EmptyState> : shares &&
      <div className="overflow-auto px-6 py-5">
        <p className="mb-4 text-xs text-muted">{shares.length} active {shares.length === 1 ? "link" : "links"}</p>
        <div className="divide-y divide-line rounded-lg border border-line">
          {shares.map((share) => <article key={share.id} className="flex flex-wrap items-center gap-4 p-4" data-testid="outgoing-share">
            <div className="min-w-0 flex-1 basis-56">
              <div className="flex items-center gap-2"><span className="rounded bg-hover px-1.5 py-0.5 text-[11px] capitalize text-muted">{share.source.kind}</span><h2 className="truncate text-sm font-medium" title={share.source.kind === "file" ? share.source.path : share.name}>{share.name}</h2></div>
              <p className="mt-1.5 truncate text-xs text-muted" title={share.workspace_name}>{share.workspace_name}{share.archived ? " · Archived source" : ""}</p>
              <p className="mt-1 text-xs text-muted" title={share.connection.state === "failed" ? share.connection.message : undefined}>Active link · {share.connection.state === "online" ? "Live" : share.connection.state === "connecting" ? "Connecting…" : "Offline"}</p>
            </div>
            <div className="flex items-center gap-2">
              <Button onClick={() => void open(share)}><ExternalLink size={13} /> Open</Button>
              <Button onClick={() => void copy(share)} title={`Copy link to ${share.name}`}><Copy size={13} /> Copy link</Button>
              <Button onClick={() => void stop(share)} disabled={stopping !== null}>{stopping === share.id ? "Stopping…" : "Stop sharing"}</Button>
            </div>
          </article>)}
        </div>
      </div>}
  </section>;
}
