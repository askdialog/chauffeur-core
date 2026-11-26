// Updating the Codex CLI from Chauffeur (`codex.update`): in Settings › Agents, and beside a failed
// turn whose model this Codex's catalog lacks (a newer Codex may offer it).

import { useEffect, useState } from "react";
import type { CodexCliStatus } from "@chauffeur/protocol";
import { Download } from "lucide-react";
import { client } from "@/state/app";
import { errorMessage } from "@/lib/rpc";
import { codexOutdated, codexStateLabel } from "@/lib/codexCli";
import { Button } from "./ui";

/** Codex's status, read on mount when `load`, followed while an update runs. */
function useCodexCli(load: boolean) {
  const [status, setStatus] = useState<CodexCliStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState(false);

  const refresh = async () => {
    try {
      setStatus(await client.call("codex.status", {}));
    } catch (err) {
      setError(errorMessage(err));
    }
  };
  useEffect(() => {
    if (load) void refresh();
  }, [load]);
  useEffect(() => {
    if (!status?.updating) return;
    // The update's end comes by asking again, as an Ollama download's does.
    const timer = setInterval(() => void refresh(), 1500);
    return () => clearInterval(timer);
  }, [status?.updating]);

  const update = async () => {
    setError(null);
    setStarted(true);
    try {
      setStatus(await client.call("codex.update", {}));
    } catch (err) {
      setError(errorMessage(err));
    }
  };
  /** The update this view started ended well. */
  const done = started && !!status && !status.updating && !status.error && !error;
  return { status, error: error ?? (status && !status.updating ? status.error : null), done, update };
}

/** Settings › Agents: the installed Codex, and its update. */
export function CodexCliSettings() {
  const { status, error, done, update } = useCodexCli(true);
  if (!status) return null;
  const offered = !!status.path && !status.updating && (codexOutdated(status) || !status.latest);
  return (
    <div className="space-y-1.5" data-testid="codex-cli">
      <div className="text-[12.5px] text-fg">Codex</div>
      <div role="status" className="text-[12px] text-muted" data-testid="codex-cli-state">
        {codexStateLabel(status)}
      </div>
      {(offered || status.updating) && (
        <Button size="sm" busy={status.updating} disabled={status.updating} onClick={() => void update()} data-testid="update-codex">
          {!status.updating && <Download size={12} />}
          {status.latest && codexOutdated(status) ? `Update Codex to ${status.latest}` : "Update Codex"}
        </Button>
      )}
      {done && (
        <p className="text-[12px] text-muted">Codex chats run the new version from their next message; a working one finishes its turn first.</p>
      )}
      {error && <p className="whitespace-pre-wrap text-[12px] text-danger selectable" data-testid="codex-cli-error">{error}</p>}
    </div>
  );
}

/** Beside a failed turn whose model this Codex lacks: update Codex, then resend. */
export function UpdateCodexRemedy() {
  const { status, error, done, update } = useCodexCli(false);
  const updating = !!status?.updating;
  return (
    <div className="mt-1.5 flex flex-col items-center gap-1 text-[12px]" data-testid="update-codex-remedy">
      {!done && (
        <Button size="sm" busy={updating} disabled={updating} onClick={() => void update()}>
          {!updating && <Download size={12} />}
          {updating ? "Updating Codex…" : "Update Codex"}
        </Button>
      )}
      {done && (
        <span className="text-muted">
          Codex {status?.version ? `${status.version} ` : ""}is installed. Resend your message.
        </span>
      )}
      {error && <span className="whitespace-pre-wrap text-danger selectable">{error}</span>}
    </div>
  );
}
