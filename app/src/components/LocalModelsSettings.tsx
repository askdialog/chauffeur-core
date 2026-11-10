import { useEffect, useState } from "react";
import type { OllamaStatus } from "@chauffeur/protocol";
import { Download, Trash2 } from "lucide-react";
import { client } from "@/state/app";
import { errorMessage } from "@/lib/rpc";
import { installLabel, pullLabel, pullPercent, sizeLabel, SUGGESTED_MODELS } from "@/lib/localModels";
import { Button, inputClass } from "./ui";

/** Settings › Local models (story 52): Ollama's state, its models, downloads and deletes. */
export function LocalModelsSettings() {
  const [status, setStatus] = useState<OllamaStatus | null>(null);
  const [name, setName] = useState("");
  const [deleting, setDeleting] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    try {
      setStatus(await client.call("ollama.status", {}));
    } catch (err) {
      setError(errorMessage(err));
    }
  };
  useEffect(() => {
    void refresh();
    // A download's progress comes by asking again, as an update's does.
    const timer = setInterval(() => void refresh(), 1500);
    return () => clearInterval(timer);
  }, []);

  async function pull(model: string) {
    setBusy(true);
    setError(null);
    try {
      await client.call("ollama.pull", { model: model.trim() });
      setName("");
      await refresh();
    } catch (err) {
      setError(`Could not download ${model.trim()}: ${errorMessage(err)}`);
    } finally {
      setBusy(false);
    }
  }

  async function install() {
    setBusy(true);
    setError(null);
    try {
      await client.call("ollama.install", {});
      await refresh();
    } catch (err) {
      setError(`Could not install Ollama: ${errorMessage(err)}`);
    } finally {
      setBusy(false);
    }
  }

  async function remove(model: string) {
    setBusy(true);
    setError(null);
    try {
      await client.call("ollama.delete", { model });
      setDeleting(null);
      await refresh();
    } catch (err) {
      setError(`Could not delete ${model}: ${errorMessage(err)}`);
    } finally {
      setBusy(false);
    }
  }

  const installed = new Set(status?.models.map((model) => model.name));
  const pulling = new Set(status?.pulls.filter((p) => !p.error).map((p) => p.model));
  const suggestions = SUGGESTED_MODELS.filter((model) => !installed.has(model.name) && !pulling.has(model.name));

  return (
    <div className="space-y-4" data-testid="local-models-settings">
      <div>
        <div className="mb-1 font-medium text-fg">Local models</div>
        <p className="text-[12px] leading-relaxed text-muted">
          A model that runs on this Mac, through Ollama, works without the network: pick it in a Codex chat's model picker. It is slower and weaker than a hosted model, so keep it for small tasks you check.
        </p>
      </div>
      {status && (
        <div role="status" data-testid="ollama-state" className="text-[12px] text-muted">
          {!status.executable ? (
            status.download_size !== null ? (
              "Ollama, which runs the models, is not installed: Chauffeur installs a copy of its own."
            ) : (
              <>
                Ollama is not installed. Install it with <code className="rounded bg-raised px-1 font-mono text-fg selectable">{status.install}</code>, then come back here.
              </>
            )
          ) : status.running === "daemon" ? (
            "Ollama runs, started by Chauffeur: it stops once no model has been used for a while."
          ) : status.running === "external" ? (
            "Ollama runs, started outside Chauffeur: Chauffeur uses it as it is."
          ) : status.own ? (
            `Ollama ${status.version} is installed by Chauffeur, which starts it when a local chat or a download needs it.`
          ) : (
            "Ollama is installed. Chauffeur starts it when a local chat or a download needs it."
          )}
        </div>
      )}
      {status && !status.executable && status.download_size !== null && (!status.installing || status.installing.error) && (
        <Button variant="primary" disabled={busy} onClick={() => void install()} data-testid="install-ollama">
          <Download size={13} /> Install Ollama {status.version} ({sizeLabel(status.download_size)})
        </Button>
      )}
      {status?.installing && (
        <div className="text-[12px]" data-testid="ollama-installing">
          <span className={status.installing.error ? "text-danger selectable" : "text-muted"}>{installLabel(status.installing)}</span>
          {!status.installing.error && status.installing.phase === "downloading" && status.installing.total > 0 && (
            <div className="mt-1 h-1 overflow-hidden rounded bg-line">
              <div className="h-full bg-accent transition-[width]" style={{ width: `${Math.min(100, (status.installing.completed * 100) / status.installing.total)}%` }} />
            </div>
          )}
        </div>
      )}
      {status && status.models.length > 0 && (
        <ul className="divide-y divide-line rounded-md border border-line bg-raised">
          {status.models.map((model) => (
            <li key={model.name} className="flex items-center justify-between gap-3 px-3 py-2 text-[12.5px]">
              <span className="min-w-0 truncate font-mono text-fg">{model.name}</span>
              <span className="flex shrink-0 items-center gap-2 text-muted">
                {sizeLabel(model.size)}
                {deleting === model.name ? (
                  <>
                    <Button size="sm" variant="danger" disabled={busy} onClick={() => void remove(model.name)}>Delete {sizeLabel(model.size)}</Button>
                    <Button size="sm" disabled={busy} onClick={() => setDeleting(null)}>Cancel</Button>
                  </>
                ) : (
                  <Button size="sm" disabled={busy} aria-label={`Delete ${model.name}`} onClick={() => setDeleting(model.name)}><Trash2 size={13} /></Button>
                )}
              </span>
            </li>
          ))}
        </ul>
      )}
      {status && status.pulls.length > 0 && (
        <ul className="space-y-2" data-testid="ollama-pulls">
          {status.pulls.map((pull) => {
            const percent = pullPercent(pull);
            return (
              <li key={pull.model} className="text-[12px]">
                <div className="flex justify-between gap-3">
                  <span className="font-mono text-fg">{pull.model}</span>
                  <span className={pull.error ? "text-danger selectable" : "text-muted"}>{pullLabel(pull)}</span>
                </div>
                {!pull.error && (
                  <div className="mt-1 h-1 overflow-hidden rounded bg-line">
                    <div className="h-full bg-accent transition-[width]" style={{ width: `${percent ?? 0}%` }} />
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {status?.executable && (
        <div className="space-y-2">
          <form
            className="flex gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              if (name.trim()) void pull(name);
            }}
          >
            <input
              aria-label="Model to download"
              className={inputClass}
              placeholder="A model's name on ollama.com, e.g. qwen3.8"
              value={name}
              onChange={(event) => setName(event.target.value)}
              spellCheck={false}
              disabled={busy}
            />
            <Button type="submit" disabled={busy || !name.trim()}><Download size={13} /> Download</Button>
          </form>
          {suggestions.map((model) => (
            <button
              key={model.name}
              type="button"
              disabled={busy}
              onClick={() => void pull(model.name)}
              className="flex w-full items-center justify-between gap-3 rounded-md border border-line px-3 py-2 text-left text-[12px] hover:bg-raised disabled:opacity-60"
            >
              <span>
                <span className="font-mono text-fg">{model.name}</span>
                <span className="block text-faint">{model.hint}</span>
              </span>
              <span className="flex shrink-0 items-center gap-1 text-muted"><Download size={12} /> {model.size}</span>
            </button>
          ))}
        </div>
      )}
      {error && <p role="alert" className="text-[12px] text-danger selectable">{error}</p>}
    </div>
  );
}
