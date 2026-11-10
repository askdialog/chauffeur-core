import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "@/state/toasts";

export function CopyTableButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
    } catch {
      toast("Could not copy the table: select its cells and copy them instead", "error");
    }
  };
  return (
    <button type="button" className="absolute right-0 top-0 flex size-6 items-center justify-center rounded text-faint opacity-0 transition-opacity hover:text-fg group-hover/table:opacity-100 focus-visible:opacity-100 focus-visible:outline-accent [@media(hover:none)]:opacity-100" onClick={() => void copy()} title={copied ? "Copied" : "Copy table for Slack or a spreadsheet"} aria-label={copied ? "Table copied" : "Copy table"} data-testid="copy-table">
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}
