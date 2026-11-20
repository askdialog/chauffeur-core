import { Download, MessageSquare } from "lucide-react";
import { type ComponentProps, useEffect, useRef, useState } from "react";
import { htmlPassageScript } from "@/components/HtmlPassages";
import { PassageComments } from "@/components/PassageComments";
import { Button, Spinner } from "@/components/ui";
import { api, ServiceError } from "./api";

// Exported HTML is self-contained. Its scripts must not access the share's session or network.
const HTML_POLICY = `<meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; form-action 'none'; base-uri 'none'">`;
type Loaded = { key: string; hash: string; preview: string; download: string; channel: string };
function release(document: Loaded | null) {
  if (!document) return;
  URL.revokeObjectURL(document.download);
  if (document.preview !== document.download) URL.revokeObjectURL(document.preview);
}

export function SharedDocument({ id, name, title, revision, onSignedOut, onRefused, review }: {
  id: string; name: string; title: string; revision: number; onSignedOut: () => void; onRefused: (message: string) => void;
  review: Omit<ComponentProps<typeof PassageComments>, "html" | "text" | "canSend" | "busy">;
}) {
  const pdf = /\.pdf$/i.test(name);
  const key = `${id}/${name}`;
  const current = useRef<Loaded | null>(null);
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const document = loaded?.key === key ? loaded : null;
  const [error, setError] = useState(false);
  const [retry, setRetry] = useState(0);
  useEffect(() => () => { release(current.current); current.current = null; }, [key]);
  useEffect(() => {
    let active = true;
    setError(false);
    void api.documentFile(id, name).then(async (file) => {
      const bytes = await file.arrayBuffer();
      const digest = await crypto.subtle.digest("SHA-256", bytes);
      const hash = Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, "0")).join("");
      if (!active || (current.current?.key === key && current.current.hash === hash)) return;
      const download = URL.createObjectURL(file);
      const channel = pdf ? "" : crypto.randomUUID();
      let preview = download;
      if (!pdf) {
        const source = new TextDecoder().decode(bytes);
        // Keep standards mode, with the policy and bridge ahead of document-supplied tags.
        const injected = HTML_POLICY + htmlPassageScript(channel);
        const doctype = /^\s*<!doctype[^>]*>/i.exec(source)?.[0];
        const html = (doctype ?? "<!doctype html>") + injected + source.slice(doctype?.length ?? 0);
        preview = URL.createObjectURL(new Blob([html], { type: "text/html;charset=utf-8" }));
      }
      const next = { key, hash, preview, download, channel };
      const previous = current.current;
      current.current = next;
      setLoaded(next);
      release(previous);
    }).catch((reason: unknown) => {
      if (!active) return;
      setError(true);
      if (reason instanceof ServiceError && [401, 403, 404, 410].includes(reason.status)) {
        release(current.current); current.current = null; setLoaded(null);
        if (reason.status === 401) onSignedOut();
        else if (reason.status !== 404) onRefused(reason.message);
      }
    });
    return () => { active = false; };
  }, [id, name, key, pdf, revision, retry, onSignedOut, onRefused]);
  return (
    <section className="flex h-full min-h-0 flex-col" aria-label="Shared document">
      <div className="flex h-10 shrink-0 items-center justify-between border-b border-line px-3 text-[12px] text-muted">
        <span>{pdf ? "PDF" : "HTML"}</span>
        {document && <div className="flex items-center gap-3">
          {!pdf && <Button size="sm" variant={review.panel ? "secondary" : "ghost"} aria-pressed={review.panel}
            onClick={() => review.onPanel(!review.panel)}><MessageSquare size={13} /> Comments</Button>}
          <a href={document.download} download={name} className="inline-flex items-center gap-1.5"><Download size={13} /> Download</a>
        </div>}
      </div>
      {error && <div className="flex shrink-0 items-center justify-center gap-3 p-3 text-[13px] text-muted">{document ? "Could not refresh this document." : "Could not open this document."}<Button onClick={() => setRetry((n) => n + 1)}>Try again</Button></div>}
      {!document ? !error && <div className="m-auto"><Spinner /></div>
        : pdf ? <iframe src={`${document.preview}#navpanes=0&view=FitH`} title={title} referrerPolicy="no-referrer" className="min-h-0 w-full flex-1 border-0 bg-white" />
        : <div className="min-h-0 flex-1">
          <PassageComments {...review} html={{ url: document.preview, title, channel: document.channel }} busy={false} canSend={false} />
        </div>}
    </section>
  );
}
