import { useEffect, useRef, useState } from "react";
import { Download } from "lucide-react";
import { Button, Spinner } from "@/components/ui";
import { api, ServiceError } from "./api";

// Exported HTML is self-contained. Its scripts must not access the share's session or network.
const HTML_POLICY = `<meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; form-action 'none'; base-uri 'none'">`;
type Loaded = { key: string; hash: string; preview: string; download: string };
function release(document: Loaded | null) {
  if (!document) return;
  URL.revokeObjectURL(document.download);
  if (document.preview !== document.download) URL.revokeObjectURL(document.preview);
}

export function SharedDocument({ id, name, title, revision, onSignedOut, onRefused }: {
  id: string; name: string; title: string; revision: number; onSignedOut: () => void; onRefused: (message: string) => void;
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
      const source = pdf ? "" : new TextDecoder().decode(bytes);
      // Keep standards mode, and put the policy before any document-supplied tags.
      const html = /^\s*<!doctype[^>]*>/i.test(source)
        ? source.replace(/^(\s*<!doctype[^>]*>)/i, `$1${HTML_POLICY}`)
        : `<!doctype html>${HTML_POLICY}${source}`;
      const download = URL.createObjectURL(file);
      const preview = pdf ? download : URL.createObjectURL(new Blob([html], { type: "text/html;charset=utf-8" }));
      const next = { key, hash, preview, download };
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
        {document && <a href={document.download} download={name} className="inline-flex items-center gap-1.5"><Download size={13} /> Download</a>}
      </div>
      {error && <div className="flex shrink-0 items-center justify-center gap-3 p-3 text-[13px] text-muted">{document ? "Could not refresh this document." : "Could not open this document."}<Button onClick={() => setRetry((n) => n + 1)}>Try again</Button></div>}
      {!document ? !error && <div className="m-auto"><Spinner /></div>
        : <iframe src={pdf ? `${document.preview}#navpanes=0&view=FitH` : document.preview} title={title} sandbox={pdf ? undefined : "allow-scripts"} referrerPolicy="no-referrer" className="min-h-0 w-full flex-1 border-0 bg-white" />}
    </section>
  );
}
