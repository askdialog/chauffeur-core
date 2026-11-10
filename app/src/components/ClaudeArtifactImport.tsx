import { useEffect, useState } from "react";
import { AlertTriangle, Loader2 } from "lucide-react";
import type { ClaudeArtifactPreview } from "@chauffeur/protocol";
import { client } from "@/state/app";
import { errorMessage } from "@/lib/rpc";

type Preview = { link: string } & (
  | { state: "ready"; value: ClaudeArtifactPreview }
  | { state: "error"; error: string }
);

export function ClaudeArtifactImport({ link, onReady }: { link: string; onReady: (link: string) => void }) {
  const [preview, setPreview] = useState<Preview | null>(null);
  useEffect(() => {
    let live = true;
    onReady("");
    const timer = setTimeout(() => {
      client.call("design.claude_preview", { url: link }).then(
        (value) => {
          if (!live) return;
          setPreview({ link, state: "ready", value });
          onReady(link);
        },
        (error: unknown) => {
          if (live) setPreview({ link, state: "error", error: errorMessage(error) });
        },
      );
    }, 400);
    return () => { live = false; clearTimeout(timer); };
  }, [link, onReady]);
  if (preview?.link !== link) {
    return <span className="flex items-center gap-1.5 text-faint" role="status"><Loader2 size={12} className="animate-spin" /> Opening a temporary Chrome window to read the public design…</span>;
  }
  if (preview.state === "error") return <span className="text-danger" role="alert">{preview.error}</span>;
  return (
    <div className="space-y-1" data-testid="claude-import-preview">
      <div className="font-medium text-fg">{preview.value.title}</div>
      <div className="text-muted">{preview.value.pages.length} pages · {preview.value.screens} screens</div>
      <div className="text-faint">{preview.value.pages.join(" · ")}</div>
      <div className="text-muted">An editable copy is imported. Claude's comments and canvas controls stay behind.</div>
      {preview.value.warnings.map((warning) => (
        <div key={warning} className="flex items-start gap-1.5 text-muted"><AlertTriangle size={13} className="mt-0.5 shrink-0 text-attention" />{warning}</div>
      ))}
    </div>
  );
}
