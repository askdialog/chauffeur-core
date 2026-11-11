import type { FileContent } from "@chauffeur/protocol";
import { Download, FileText, Maximize2, Minimize2, RefreshCw, Share2, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import { deliverableDownloadUrl, deliverableFormat, type Deliverable } from "@/lib/deliverables";
import { openExternal } from "@/lib/native";
import { errorMessage } from "@/lib/rpc";
import { client, toast, useApp } from "@/state/app";
import { useFileShare } from "./FileComments";
import { ShareDialog } from "./design/ShareDialog";
import { DeliverableContext } from "./DeliverableCard";
import { Button, EmptyState, IconButton, Spinner } from "./ui";

/** The reader belongs to this conversation; closing it never changes the chat or its draft. */
export function Deliverables({ workspaceId, revision, children }: { workspaceId: string; revision: number; children: ReactNode }) {
  const [document, setDocument] = useState<Deliverable | null>(null);
  const [expanded, setExpanded] = useState(false);
  return (
    <DeliverableContext.Provider value={setDocument}>
      <div className="@container flex min-h-0 min-w-0 flex-1">
        <div className="flex min-h-0 min-w-0 flex-1 flex-col @min-[850px]:flex-row">
          <div className={`${expanded && document ? "hidden" : "flex"} min-h-0 min-w-0 flex-1 flex-col`}>{children}</div>
          {document && (
            <div className={`${expanded ? "flex-1" : "h-1/2 @min-[850px]:h-auto @min-[850px]:w-[56%]"} flex min-h-0 min-w-0 flex-col border-l border-line bg-panel`}>
              <DeliverableReader
                key={document.path}
                workspaceId={workspaceId}
                document={document}
                revision={revision}
                expanded={expanded}
                onExpand={() => setExpanded(!expanded)}
                onClose={() => { setDocument(null); setExpanded(false); }}
              />
            </div>
          )}
        </div>
      </div>
    </DeliverableContext.Provider>
  );
}

function DeliverableReader({ workspaceId, document, revision, expanded, onExpand, onClose }: {
  workspaceId: string; document: Deliverable; revision: number; expanded: boolean; onExpand: () => void; onClose: () => void;
}) {
  const [selection, setPath] = useState(document.path);
  const formats = [document.path, ...document.exports];
  const path = formats.includes(selection) ? selection : document.path;
  const [loaded, setLoaded] = useState<{ path: string; content: FileContent } | null>(null);
  const content = loaded?.path === path ? loaded.content : null;
  const [error, setError] = useState<string | null>(null);
  const [reload, setReload] = useState(0);
  const [downloading, setDownloading] = useState(false);
  const [sharing, setSharing] = useState(false);
  const share = useFileShare(workspaceId, path);
  const connection = useApp((s) => s.connection.status);
  useEffect(() => {
    if (connection !== "open") return;
    let active = true;
    setError(null);
    void client.call("files.read", { workspace: workspaceId, path }).then((next) => {
      if (active) setLoaded({ path, content: next });
    }).catch((reason: unknown) => { if (active) setError(errorMessage(reason)); });
    return () => { active = false; };
  }, [workspaceId, path, revision, connection, reload]);

  const pdf = deliverableFormat(path) === "PDF";
  const media = client.httpUrl(`/media/${encodeURIComponent(workspaceId)}`);
  const download = client.httpUrl(`/deliverables/${encodeURIComponent(workspaceId)}`);
  const url = content && (pdf
    ? content.kind === "media" && content.mime === "application/pdf" && media
      ? `${media}&path=${encodeURIComponent(path)}&version=${encodeURIComponent(content.version)}#navpanes=0&view=FitH`
      : null
    : content.preview_url);
  return (
    <section className="flex min-h-0 flex-1 flex-col" aria-label="Document preview" data-testid="deliverable-preview">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-line px-3">
        <FileText size={15} className="shrink-0 text-muted" />
        <span className="min-w-0 flex-1 truncate text-[13px] font-medium">{document.title}</span>
        <IconButton title="Reload document" onClick={() => setReload(reload + 1)}><RefreshCw size={14} /></IconButton>
        <IconButton title={expanded ? "Show conversation" : "Expand document"} onClick={onExpand}>{expanded ? <Minimize2 size={14} /> : <Maximize2 size={14} />}</IconButton>
        <IconButton title="Close preview" onClick={onClose}><X size={15} /></IconButton>
      </header>
      <div className="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2">
        {formats.length === 1 ? <span className="text-[11px] text-muted">{deliverableFormat(path)}</span> : formats.map((file) => <Button key={file} size="sm" variant={file === path ? "secondary" : "ghost"} aria-pressed={file === path} onClick={() => setPath(file)}>{deliverableFormat(file)}</Button>)}
        {content && download && url && <Button className="ml-auto" size="sm" variant="ghost" busy={downloading} onClick={() => {
          setDownloading(true);
          // The system browser gets a one-document capability, never the daemon's connection token.
          void client.call("files.download", { workspace: workspaceId, path })
            .then((ticket) => openExternal(deliverableDownloadUrl(download, ticket)))
            .catch((reason: unknown) => toast(`Could not download ${document.title}: ${errorMessage(reason)}`, "error"))
            .finally(() => setDownloading(false));
        }}><Download size={13} /> Download {deliverableFormat(path)}</Button>}
        <Button size="sm" variant="ghost" onClick={() => setSharing(true)} disabled={!content || !!error}><Share2 size={13} /> Share</Button>
      </div>
      {sharing && <ShareDialog key={path} target={{ kind: "file", workspace: workspaceId, path }} state={share} onClose={() => setSharing(false)} />}
      {error ? <EmptyState title="Could not open this document"><p>{error}</p><Button onClick={() => setReload(reload + 1)}>Try again</Button></EmptyState>
        : !content ? <div className="flex flex-1 items-center justify-center gap-2 text-[12px] text-muted" role="status"><Spinner size={14} /> Loading document…</div>
        : !url ? <EmptyState title="Preview unavailable">Ask the agent to regenerate this HTML or PDF document.</EmptyState>
        : <iframe key={`${path}:${content.version}`} src={url} title={document.title} className="min-h-0 w-full flex-1 border-0 bg-white"
            sandbox={pdf ? undefined : "allow-scripts"} referrerPolicy="no-referrer" />}
      <footer className="shrink-0 border-t border-line px-3 py-2 text-[11px] text-muted">Ask for changes in this conversation. The preview refreshes after each turn.</footer>
    </section>
  );
}
