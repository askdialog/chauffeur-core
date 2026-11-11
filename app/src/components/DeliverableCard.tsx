import { FileText } from "lucide-react";
import { createContext, useContext } from "react";
import { deliverableFormat, type Deliverable } from "@/lib/deliverables";
import { Button } from "./ui";

export const DeliverableContext = createContext<((document: Deliverable) => void) | null>(null);

export function DeliverableCard({ document, streaming }: { document: Deliverable; streaming: boolean }) {
  const open = useContext(DeliverableContext);
  return (
    <div className="my-3 flex items-center gap-3 rounded-lg border border-line bg-raised p-3" data-testid="deliverable-card">
      <FileText size={22} className="shrink-0 text-accent" />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-medium text-fg">{document.title}</div>
        <div className="mt-1 text-[11px] text-muted">{[document.path, ...document.exports].map(deliverableFormat).join(" · ")}</div>
      </div>
      {open ? <Button disabled={streaming} onClick={() => open(document)}>{streaming ? "Preparing…" : "Open preview"}</Button>
        : <span className="text-[11px] text-muted">Open in the workspace to preview</span>}
    </div>
  );
}

