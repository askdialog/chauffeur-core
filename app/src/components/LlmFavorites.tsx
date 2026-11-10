import type { LoadoutEntry } from "@chauffeur/protocol";
import { ArrowDown, ArrowUp, Plus, X } from "lucide-react";
import { useState } from "react";
import {
  effectiveLoadout,
  effortLabel,
  findModel,
  modelName,
  moveEntry,
  searchModels,
  toEntry,
  withModel,
} from "@/lib/models";
import { kbd } from "@/lib/keys";
import { saveLoadout, useApp } from "@/state/app";
import { AgentIcon } from "./AgentIcon";
import { Button, inputClass, Select, Spinner } from "./ui";

/** Each favorite keeps its own reasoning and speed; the first is the default. */
export function LlmFavorites() {
  const catalog = useApp((s) => s.models);
  const saved = useApp((s) => s.loadout);
  const [busy, setBusy] = useState(false);
  const [adding, setAdding] = useState(false);
  const [query, setQuery] = useState("");
  const entries = effectiveLoadout(saved, catalog);
  const hits = searchModels(catalog, query);
  const persist = async (next: LoadoutEntry[]) => {
    setBusy(true);
    try {
      await saveLoadout(next);
    } finally {
      setBusy(false);
    }
  };
  const replace = (index: number, entry: LoadoutEntry) =>
    void persist(entries.map((current, i) => (i === index ? entry : current)));
  return (
    <div className="space-y-3">
      <p className="text-[12px] leading-relaxed text-muted">
        The first favorite is the default for new workspaces and chats. Edit
        each model’s reasoning and speed below. The order sets the chat picker’s
        shortcuts.
      </p>
      {!catalog || saved == null ? (
        <Spinner />
      ) : (
        <div
          className="rounded-lg border border-line"
          data-testid="llm-favorites"
        >
          {entries.map((entry, index) => {
            const model = findModel(catalog, entry.agent, entry.model);
            const name = modelName(catalog, entry);
            return (
              <div
                key={index}
                className="space-y-2 border-b border-line p-3 last:border-b-0"
                data-testid={`llm-favorite-${index}`}
              >
                <div className="flex items-center gap-2">
                  <span
                    className="text-[10px] text-faint"
                    title="Chat picker shortcut"
                  >
                    {index < 9 ? kbd(`⌃⌘${index + 1}`) : index + 1}
                  </span>
                  <AgentIcon agent={entry.agent} size={14} />
                  <span className="min-w-0 flex-1 truncate text-[12.5px] text-fg">
                    {name}
                  </span>
                  {index === 0 ? (
                    <span className="text-[11px] text-accent">Default</span>
                  ) : (
                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void persist(moveEntry(entries, index, 0))}
                      className="text-[11px] text-muted hover:text-fg"
                    >
                      Make default
                    </button>
                  )}
                  <button
                    type="button"
                    aria-label={`Move ${name} up`}
                    disabled={busy || index === 0}
                    onClick={() =>
                      void persist(moveEntry(entries, index, index - 1))
                    }
                    className="rounded p-1 text-muted hover:bg-hover disabled:opacity-30"
                  >
                    <ArrowUp size={13} />
                  </button>
                  <button
                    type="button"
                    aria-label={`Move ${name} down`}
                    disabled={busy || index === entries.length - 1}
                    onClick={() =>
                      void persist(moveEntry(entries, index, index + 1))
                    }
                    className="rounded p-1 text-muted hover:bg-hover disabled:opacity-30"
                  >
                    <ArrowDown size={13} />
                  </button>
                  <button
                    type="button"
                    aria-label={`Remove ${name}`}
                    disabled={busy || entries.length === 1}
                    onClick={() =>
                      void persist(entries.filter((_, i) => i !== index))
                    }
                    className="rounded p-1 text-muted hover:bg-hover disabled:opacity-30"
                  >
                    <X size={13} />
                  </button>
                </div>
                <div className="flex items-center gap-4 text-[11.5px] text-muted">
                  <div className="flex items-center gap-2">
                    <span>Reasoning</span>
                    <Select
                      title={`Reasoning for ${name}`}
                      testId={`llm-reasoning-${index}`}
                      className="border border-line bg-bg !h-7 !px-2"
                      width={210}
                      disabled={busy || !model?.efforts.length}
                      value={entry.effort ?? ""}
                      options={[
                        {
                          value: "",
                          label: model?.default_effort
                            ? `Model default (${effortLabel(model.default_effort)})`
                            : "Model default",
                        },
                        ...(entry.effort &&
                        !model?.efforts.includes(entry.effort)
                          ? [
                              {
                                value: entry.effort,
                                label: effortLabel(entry.effort),
                              },
                            ]
                          : []),
                        ...(model?.efforts ?? []).map((effort) => ({
                          value: effort,
                          label: effortLabel(effort),
                        })),
                      ]}
                      onChange={(effort) =>
                        replace(index, {
                          ...entry,
                          effort: effort || undefined,
                        })
                      }
                    />
                  </div>
                  <label
                    className="flex items-center gap-1.5"
                    title={
                      model?.supports_fast
                        ? "Faster responses, more usage"
                        : "Fast mode is unavailable for this model"
                    }
                  >
                    <input
                      type="checkbox"
                      aria-label={`Fast mode for ${name}`}
                      checked={entry.fast ?? false}
                      disabled={busy || !model?.supports_fast}
                      onChange={(event) =>
                        replace(index, { ...entry, fast: event.target.checked })
                      }
                    />
                    Fast
                  </label>
                </div>
              </div>
            );
          })}
        </div>
      )}
      <Button
        disabled={busy || !catalog || saved == null}
        onClick={() => setAdding(!adding)}
      >
        <Plus size={12} /> Add a favorite
      </Button>
      {adding && (
        <div className="space-y-2 rounded-lg border border-line p-3">
          <input
            aria-label="Search models to add"
            placeholder="Search models…"
            className={inputClass}
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
          <div className="max-h-40 overflow-y-auto">
            {hits.map(({ agent, model }) => (
              <button
                key={`${agent}:${model.id}`}
                type="button"
                disabled={busy}
                className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-[12px] text-muted hover:bg-hover hover:text-fg"
                onClick={() => {
                  const entry = toEntry(catalog, withModel(null, agent, model));
                  if (entry) void persist([...entries, entry]);
                  setAdding(false);
                  setQuery("");
                }}
              >
                <AgentIcon agent={agent} size={12} />
                {model.name}
              </button>
            ))}
          </div>
          {hits.length === 0 && (
            <p className="text-[12px] text-faint">No model matches.</p>
          )}
        </div>
      )}
    </div>
  );
}
