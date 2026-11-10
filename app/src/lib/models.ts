// The model picker's logic: what a chat runs on (agent, model, effort, fast), matched against the
// agents' catalogs and the user's loadout of favorites.

import type { AgentKind, AgentModel, AgentModels, LoadoutEntry } from "@chauffeur/protocol";

/** What a chat runs on. A null model or effort is the agent's default. */
export type ModelChoice = {
  agent: AgentKind;
  model: string | null;
  effort: string | null;
  fast: boolean;
};

export const AGENT_NAMES: Record<AgentKind, string> = { claude: "Claude Code", codex: "Codex" };

const EFFORT_LABELS: Record<string, string> = {
  low: "Low",
  medium: "Medium",
  high: "High",
  xhigh: "Extra high",
  max: "Max",
  ultra: "Ultra",
};

export function effortLabel(effort: string): string {
  return EFFORT_LABELS[effort] ?? effort.charAt(0).toUpperCase() + effort.slice(1);
}

export function modelsOf(catalog: AgentModels[] | null, agent: AgentKind): AgentModel[] {
  return catalog?.find((c) => c.agent === agent)?.models ?? [];
}

/**
 * The catalog entry for a chat's model: by id, or by the id it resolves to (a chat reports
 * `claude-opus-5-5` once its session started on `opus`). A null model is the agent's default.
 */
export function findModel(catalog: AgentModels[] | null, agent: AgentKind, model: string | null): AgentModel | null {
  const models = modelsOf(catalog, agent);
  if (!model) return models.find((m) => m.is_default) ?? null;
  return models.find((m) => m.id === model) ?? models.find((m) => m.resolved === model) ?? null;
}

/** The name to show for a choice, even before the catalog is known. */
export function modelName(catalog: AgentModels[] | null, choice: Pick<ModelChoice, "agent" | "model">): string {
  const found = findModel(catalog, choice.agent, choice.model);
  if (found) return found.name;
  if (choice.model) return choice.model;
  return `${AGENT_NAMES[choice.agent]} default`;
}

/** The effort shown for a choice: its own, else the model's default when the agent says. */
export function shownEffort(catalog: AgentModels[] | null, choice: ModelChoice): string | null {
  if (choice.effort) return choice.effort;
  return findModel(catalog, choice.agent, choice.model)?.default_effort ?? null;
}

/** Moving to another model keeps the effort and fast mode when the new model has them. */
export function withModel(choice: ModelChoice | null, agent: AgentKind, model: AgentModel): ModelChoice {
  const effort =
    choice?.effort && model.efforts.includes(choice.effort) ? choice.effort : (model.default_effort ?? null);
  return {
    agent,
    model: model.id,
    effort: model.efforts.length > 0 ? effort : null,
    fast: model.supports_fast && (choice?.fast ?? false),
  };
}

/** The next effort level of the model, wrapping around. */
export function cycleEffort(catalog: AgentModels[] | null, choice: ModelChoice): ModelChoice {
  const model = findModel(catalog, choice.agent, choice.model);
  const efforts = model?.efforts ?? [];
  if (efforts.length === 0) return choice;
  const current = shownEffort(catalog, choice);
  const index = current ? efforts.indexOf(current) : -1;
  return { ...choice, effort: efforts[(index + 1) % efforts.length] ?? null };
}

export function fromEntry(entry: LoadoutEntry): ModelChoice {
  return { agent: entry.agent, model: entry.model, effort: entry.effort ?? null, fast: entry.fast ?? false };
}

export function toEntry(catalog: AgentModels[] | null, choice: ModelChoice): LoadoutEntry | null {
  const model = findModel(catalog, choice.agent, choice.model);
  const id = model?.id ?? choice.model;
  if (!id) return null;
  return { agent: choice.agent, model: id, effort: choice.effort ?? undefined, fast: choice.fast };
}

/** Whether a choice is this loadout entry (the same model, effort and speed). */
export function isEntry(catalog: AgentModels[] | null, entry: LoadoutEntry, choice: ModelChoice): boolean {
  if (entry.agent !== choice.agent || (entry.fast ?? false) !== choice.fast) return false;
  const a = findModel(catalog, entry.agent, entry.model);
  const b = findModel(catalog, choice.agent, choice.model);
  const sameModel = a && b ? a.id === b.id : entry.model === choice.model;
  return sameModel && (entry.effort ?? null) === shownEffort(catalog, choice);
}

/** Until the user saves a loadout: each agent's default model at a sensible effort. */
export function defaultLoadout(catalog: AgentModels[] | null): LoadoutEntry[] {
  const entries: LoadoutEntry[] = [];
  for (const agent of ["claude", "codex"] as const) {
    const model = findModel(catalog, agent, null);
    if (!model) continue;
    const effort = model.default_effort ?? (model.efforts.includes("high") ? "high" : undefined);
    entries.push({ agent, model: model.id, effort, fast: false });
  }
  return entries;
}

/** The loadout in effect: the saved one, else the default one. */
export function effectiveLoadout(saved: LoadoutEntry[] | null, catalog: AgentModels[] | null): LoadoutEntry[] {
  return saved && saved.length > 0 ? saved : defaultLoadout(catalog);
}

/**
 * The loadout another machine can run: the saved entries its catalog has (by model, or the
 * agent's default when an entry names none), else its own defaults.
 */
export function loadoutIn(saved: LoadoutEntry[] | null, catalog: AgentModels[] | null): LoadoutEntry[] {
  const runnable = (saved ?? []).filter((entry) => findModel(catalog, entry.agent, entry.model) != null);
  return runnable.length > 0 ? runnable : defaultLoadout(catalog);
}

/** The choice a new chat or workspace starts with: the first loadout entry. */
export function initialChoice(loadout: LoadoutEntry[]): ModelChoice {
  const first = loadout[0];
  return first ? fromEntry(first) : { agent: "claude", model: null, effort: null, fast: false };
}

/** The choice a design chat starts with: the first Claude entry of the loadout (design chats run
 * on Claude Code only), else Claude Code's own default. */
export function designChoice(loadout: LoadoutEntry[]): ModelChoice {
  const first = loadout.find((entry) => entry.agent === "claude");
  return first ? fromEntry(first) : { agent: "claude", model: null, effort: null, fast: false };
}

export type SearchHit = { agent: AgentKind; model: AgentModel };

/** Every model of every agent whose name, id or description holds each word of the query. */
export function searchModels(catalog: AgentModels[] | null, query: string): SearchHit[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const hits: SearchHit[] = [];
  for (const { agent, models } of catalog ?? []) {
    for (const model of models) {
      const haystack = `${model.name} ${model.id} ${model.description} ${AGENT_NAMES[agent]}`.toLowerCase();
      if (words.every((word) => haystack.includes(word))) hits.push({ agent, model });
    }
  }
  return hits;
}

export function moveEntry<T>(list: T[], from: number, to: number): T[] {
  if (from === to || from < 0 || from >= list.length) return list;
  const next = [...list];
  const [item] = next.splice(from, 1);
  next.splice(Math.max(0, Math.min(to, next.length)), 0, item as T);
  return next;
}
