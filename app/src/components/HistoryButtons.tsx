import { ArrowLeft, ArrowRight } from "lucide-react";
import { kbd } from "@/lib/keys";
import { canGoInHistory, goInHistory, useApp } from "@/state/app";
import { IconButton } from "./ui";

/** Back to the workspace shown before this one, and forward again. */
export function HistoryButtons() {
  const canGoBack = useApp((s) => canGoInHistory(s, -1));
  const canGoForward = useApp((s) => canGoInHistory(s, 1));
  return (
    <>
      <IconButton title={`Back to the previous workspace (${kbd("⌘[")})`} disabled={!canGoBack} onClick={() => goInHistory(-1)}>
        <ArrowLeft size={14} />
      </IconButton>
      <IconButton title={`Forward to the next workspace (${kbd("⌘]")})`} disabled={!canGoForward} onClick={() => goInHistory(1)}>
        <ArrowRight size={14} />
      </IconButton>
    </>
  );
}
