import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "@/state/toasts";

/** Copies a code block's source as written, without its highlighting. */
export function CopyCodeButton({ text }: { text: string }) {
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
      toast("Could not copy the code", "error");
    }
  };
  return (
    <button type="button" className="md-block-button" onClick={() => void copy()} title="Copy the code" aria-label="Copy the code">
      {copied ? <Check size={12} /> : <Copy size={12} />}
      {copied ? "Copied" : "Copy"}
    </button>
  );
}
