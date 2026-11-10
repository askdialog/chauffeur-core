import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import { copyMarkdown } from "@/lib/copyMarkdown";
import { toast } from "@/state/toasts";

export function CopyMessageButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);
  const copy = async () => {
    try {
      await copyMarkdown(text);
      setCopied(true);
    } catch {
      toast("Could not copy the response: select its text and copy it instead", "error");
    }
  };
  return (
    <button type="button" className="mt-1 flex size-6 items-center justify-center rounded text-faint hover:text-fg focus-visible:outline-accent" onClick={() => void copy()} title={copied ? "Copied" : "Copy the response"} aria-label={copied ? "Response copied" : "Copy the response"} data-testid="copy-response">
      {copied ? <Check size={12} /> : <Copy size={12} />}
    </button>
  );
}
