// What to do about an MCP server that lacks environment variables (decided with Justin
// 2026-10-18): export each in the login shell's profile (a line to copy; the user types the
// value, which Chauffeur never sees), then reload the environment; or, when the server is not
// wanted, remove it where it is declared.

import type { McpServer } from "@chauffeur/protocol";
import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { exportLine, profileName } from "@/lib/connectors";
import { reloadEnvironment, toast, useApp } from "@/state/app";
import { Button } from "./ui";

export function MissingEnvHelp({ server }: { server: McpServer }) {
  const profile = useApp((s) => (s.connection.status === "open" ? s.connection.shellProfile : undefined));
  const [busy, setBusy] = useState(false);
  const reload = async () => {
    setBusy(true);
    try {
      await reloadEnvironment();
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="flex min-w-0 flex-col gap-1 text-[11px]" data-testid="mcp-missing-env" data-server={server.name}>
      <span className="text-faint">
        Add {server.missing_env.length === 1 ? "this line" : "these lines"} to <span className="font-mono text-muted">{profileName(profile)}</span>, with
        the value, then reload:
      </span>
      {server.missing_env.map((variable) => (
        <CopyLine key={variable} text={exportLine(variable)} />
      ))}
      <span className="flex items-center gap-2">
        <Button size="xs" variant="primary" busy={busy} onClick={() => void reload()}>
          Reload environment
        </Button>
      </span>
      {server.declared_in && (
        <span className="break-words text-faint" data-testid="mcp-declared-in">
          Not needed? Remove it from <span className="font-mono text-muted">{server.declared_in}</span>.
        </span>
      )}
    </div>
  );
}

function CopyLine({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      toast("Could not copy the line", "error");
    }
  };
  return (
    <span className="flex min-w-0 items-center gap-1 rounded border border-line bg-raised py-0.5 pl-1.5 pr-0.5">
      <code className="min-w-0 flex-1 truncate font-mono text-[11px] text-fg" title={text}>
        {text}
      </code>
      <button
        type="button"
        onClick={() => void copy()}
        title="Copy the line"
        aria-label={`Copy ${text}`}
        className="shrink-0 rounded p-0.5 text-faint hover:bg-hover hover:text-fg"
      >
        {copied ? <Check size={11} /> : <Copy size={11} />}
      </button>
    </span>
  );
}
