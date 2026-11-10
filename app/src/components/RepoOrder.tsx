import type { RepoInfo } from "@chauffeur/protocol";
import { ArrowDown, ArrowUp, GripVertical } from "lucide-react";
import { useState } from "react";
import { moveEntry } from "@/lib/models";
import { Button, Field, inputClass } from "./ui";

export function RepoOrder({ repos, order, busy, error, onRefresh, onSave }: {
  repos: RepoInfo[] | null;
  error: string | null;
  onRefresh: () => Promise<void>;
  order: string[];
  busy: boolean;
  onSave: (order: string[]) => void;
}) {
  const [refreshing, setRefreshing] = useState(false);
  const disabled = busy || refreshing;
  const [filter, setFilter] = useState("");
  const [dragged, setDragged] = useState<string | null>(null);
  const names = (repos ?? []).map((repo) => repo.key);
  const move = (name: string, target: string) => {
    const from = names.indexOf(name);
    const to = names.indexOf(target);
    if (disabled || from < 0 || to < 0 || from === to) return;
    onSave([...moveEntry(names, from, to), ...order.filter((repo) => !names.includes(repo))]);
  };
  return (
    <Field label="Repo display order" hint="used when creating a workspace">
      <div className="space-y-2" data-testid="repo-order">
        <div className="flex items-center gap-2">
          <input className={inputClass} placeholder="Filter repos…" aria-label="Filter repo order"
            value={filter} onChange={(event) => setFilter(event.target.value)} />
          <Button size="sm" disabled={disabled} onClick={async () => {
            setRefreshing(true);
            try { await onRefresh(); } finally { setRefreshing(false); }
          }}>Refresh</Button>
          <Button size="sm" disabled={disabled || order.length === 0} onClick={() => onSave([])}>Reset</Button>
        </div>
        <p className="text-[11px] text-faint">Drag repos or use the arrows. New repos appear after your saved order.</p>
        <div className="max-h-52 overflow-y-auto rounded-md border border-line" aria-label="Repo display order">
          {error && <p className="px-3 py-2 text-[12px] text-danger">Could not load repos: {error}. Refresh to try again.</p>}
          {repos == null && !error && <p className="px-3 py-2 text-[12px] text-faint">Loading repos…</p>}
          {repos != null && repos.length > 0 && !names.some((name) => name.toLowerCase().includes(filter.trim().toLowerCase())) && (
            <p className="px-3 py-2 text-[12px] text-faint">No repo matches “{filter}”.</p>
          )}
          {repos?.length === 0 && <p className="px-3 py-2 text-[12px] text-faint">No repos available.</p>}
          {repos?.filter((repo) => repo.key.toLowerCase().includes(filter.trim().toLowerCase())).map((repo) => {
            const index = names.indexOf(repo.key);
            return (
              <div key={repo.key} data-ordered-repo={repo.key}
                draggable={!disabled}
                onDragStart={(event) => { event.dataTransfer.setData("text/plain", repo.key); setDragged(repo.key); }}
                onDragEnd={() => setDragged(null)}
                onDragOver={(event) => { if (dragged && !disabled) event.preventDefault(); }}
                onDrop={(event) => { event.preventDefault(); if (dragged) move(dragged, repo.key); setDragged(null); }}
                className="flex items-center gap-2 px-2 py-1 text-[12px] hover:bg-hover">
                <GripVertical size={12} className="shrink-0 cursor-grab text-faint" />
                <span className="min-w-0 flex-1 truncate">{repo.key}</span>
                {repo.is_default && <span className="text-[10.5px] text-faint">default</span>}
                <button type="button" aria-label={`Move ${repo.key} up`} disabled={disabled || index === 0}
                  className="rounded p-1 text-muted hover:bg-hover disabled:opacity-30"
                  onClick={() => move(repo.key, names[index - 1])}><ArrowUp size={12} /></button>
                <button type="button" aria-label={`Move ${repo.key} down`} disabled={disabled || index === names.length - 1}
                  className="rounded p-1 text-muted hover:bg-hover disabled:opacity-30"
                  onClick={() => move(repo.key, names[index + 1])}><ArrowDown size={12} /></button>
              </div>
            );
          })}
        </div>
      </div>
    </Field>
  );
}
