import { describe, expect, test } from "bun:test";
import type { AgentModel, AgentModels } from "@chauffeur/protocol";
import {
  cycleEffort,
  defaultLoadout,
  effectiveLoadout,
  effortLabel,
  findModel,
  designChoice,
  initialChoice,
  isEntry,
  loadoutIn,
  modelName,
  moveEntry,
  searchModels,
  shownEffort,
  toEntry,
  withModel,
} from "./models";

const model = (overrides: Partial<AgentModel>): AgentModel => ({
  id: "x",
  resolved: null,
  name: "X",
  description: "",
  efforts: [],
  default_effort: null,
  supports_fast: false,
  is_default: false,
  ...overrides,
});

const OPUS = model({ id: "opus", resolved: "claude-opus-5-5", name: "Opus 5.5", efforts: ["low", "medium", "high", "xhigh", "max"], supports_fast: true, is_default: true });
const SONNET = model({ id: "sonnet", resolved: "claude-sonnet-5-5", name: "Sonnet 5.5", efforts: ["low", "medium", "high"] });
const HAIKU = model({ id: "claude-haiku-4-5-20251001", resolved: "claude-haiku-4-5-20251001", name: "Haiku 4.5", description: "Fastest" });
const SOL = model({ id: "gpt-6.1-sol", resolved: "gpt-6.1-sol", name: "GPT-6.1 Sol", efforts: ["low", "medium", "high", "ultra"], default_effort: "low", supports_fast: true, is_default: true });
const LUNA = model({ id: "gpt-6-luna", resolved: "gpt-6-luna", name: "GPT-6 Luna", description: "Fast and affordable", efforts: ["low", "medium"], default_effort: "medium" });

const CATALOG: AgentModels[] = [
  { agent: "claude", models: [OPUS, SONNET, HAIKU], error: null },
  { agent: "codex", models: [SOL, LUNA], error: null },
];

describe("the loadout another machine runs", () => {
  test("keeps the favorites its catalog has, else its defaults", () => {
    const there: AgentModels[] = [{ agent: "claude", models: [OPUS], error: null }];
    const saved = [
      { agent: "claude" as const, model: "sonnet", fast: false },
      { agent: "claude" as const, model: "opus", effort: "high", fast: false },
    ];
    expect(loadoutIn(saved, there)).toEqual([{ agent: "claude", model: "opus", effort: "high", fast: false }]);
    // None of them there: its own defaults.
    expect(loadoutIn([saved[0]], there)).toEqual(defaultLoadout(there));
    // Not read yet: nothing to offer.
    expect(loadoutIn(saved, null)).toEqual([]);
  });
});

describe("finding a chat's model", () => {
  test("by id, by the id it resolved to, or the agent's default", () => {
    expect(findModel(CATALOG, "claude", "sonnet")?.name).toBe("Sonnet 5.5");
    expect(findModel(CATALOG, "claude", "claude-opus-5-5")?.name).toBe("Opus 5.5");
    expect(findModel(CATALOG, "codex", null)?.name).toBe("GPT-6.1 Sol");
    expect(findModel(CATALOG, "codex", "unknown")).toBeNull();
  });

  test("names what the catalog does not know yet", () => {
    expect(modelName(null, { agent: "claude", model: null })).toBe("Claude Code default");
    expect(modelName(null, { agent: "codex", model: "gpt-7" })).toBe("gpt-7");
    expect(modelName(CATALOG, { agent: "claude", model: null })).toBe("Opus 5.5");
  });

  test("shows the model's default effort when the chat sets none", () => {
    expect(shownEffort(CATALOG, { agent: "codex", model: "gpt-6-luna", effort: null, fast: false })).toBe("medium");
    expect(shownEffort(CATALOG, { agent: "claude", model: "opus", effort: null, fast: false })).toBeNull();
    expect(effortLabel("xhigh")).toBe("Extra high");
    expect(effortLabel("turbo")).toBe("Turbo");
  });
});

describe("changing the choice", () => {
  test("another model keeps the effort and fast mode it supports", () => {
    const opusMax = { agent: "claude" as const, model: "opus", effort: "max", fast: true };
    expect(withModel(opusMax, "claude", SONNET)).toEqual({ agent: "claude", model: "sonnet", effort: null, fast: false });
    expect(withModel({ ...opusMax, effort: "high" }, "claude", SONNET)).toEqual({ agent: "claude", model: "sonnet", effort: "high", fast: false });
    expect(withModel(opusMax, "claude", HAIKU)).toEqual({ agent: "claude", model: HAIKU.id, effort: null, fast: false });
    expect(withModel(opusMax, "codex", SOL)).toEqual({ agent: "codex", model: "gpt-6.1-sol", effort: "low", fast: true });
  });

  test("effort cycles through the model's levels from the shown one", () => {
    const luna = { agent: "codex" as const, model: "gpt-6-luna", effort: null, fast: false };
    expect(cycleEffort(CATALOG, luna).effort).toBe("low"); // medium (default) → wraps to low
    expect(cycleEffort(CATALOG, { ...luna, effort: "low" }).effort).toBe("medium");
    const haiku = { agent: "claude" as const, model: HAIKU.id, effort: null, fast: false };
    expect(cycleEffort(CATALOG, haiku)).toBe(haiku);
  });
});

describe("the loadout", () => {
  test("defaults to each agent's default model until one is saved", () => {
    expect(defaultLoadout(CATALOG)).toEqual([
      { agent: "claude", model: "opus", effort: "high", fast: false },
      { agent: "codex", model: "gpt-6.1-sol", effort: "low", fast: false },
    ]);
    const saved = [{ agent: "codex" as const, model: "gpt-6-luna", fast: true }];
    expect(effectiveLoadout(saved, CATALOG)).toBe(saved);
    expect(effectiveLoadout([], CATALOG)).toHaveLength(2);
    expect(initialChoice(saved)).toEqual({ agent: "codex", model: "gpt-6-luna", effort: null, fast: true });
    expect(initialChoice([])).toEqual({ agent: "claude", model: null, effort: null, fast: false });
  });

  test("a design chat starts on the first Claude entry", () => {
    const saved = [
      { agent: "codex" as const, model: "gpt-6-luna", fast: true },
      { agent: "claude" as const, model: "opus", effort: "high", fast: false },
    ];
    expect(designChoice(saved)).toEqual({ agent: "claude", model: "opus", effort: "high", fast: false });
    expect(designChoice([saved[0]])).toEqual({ agent: "claude", model: null, effort: null, fast: false });
  });

  test("an entry matches a chat on the resolved model, the shown effort and the speed", () => {
    const entry = { agent: "claude" as const, model: "opus", effort: "high", fast: false };
    expect(isEntry(CATALOG, entry, { agent: "claude", model: "claude-opus-5-5", effort: "high", fast: false })).toBe(true);
    expect(isEntry(CATALOG, entry, { agent: "claude", model: "opus", effort: "high", fast: true })).toBe(false);
    expect(isEntry(CATALOG, entry, { agent: "claude", model: "opus", effort: "low", fast: false })).toBe(false);
    const luna = { agent: "codex" as const, model: "gpt-6-luna", effort: "medium", fast: false };
    expect(isEntry(CATALOG, luna, { agent: "codex", model: "gpt-6-luna", effort: null, fast: false })).toBe(true);
  });

  test("a choice becomes an entry under the catalog's id", () => {
    expect(toEntry(CATALOG, { agent: "claude", model: "claude-opus-5-5", effort: "max", fast: true })).toEqual({
      agent: "claude",
      model: "opus",
      effort: "max",
      fast: true,
    });
    expect(toEntry(null, { agent: "claude", model: null, effort: null, fast: false })).toBeNull();
  });

  test("entries move", () => {
    expect(moveEntry(["a", "b", "c"], 0, 2)).toEqual(["b", "c", "a"]);
    expect(moveEntry(["a", "b", "c"], 2, 0)).toEqual(["c", "a", "b"]);
    expect(moveEntry(["a", "b"], 1, 1)).toEqual(["a", "b"]);
  });
});

describe("search", () => {
  test("matches every word against name, id, description and agent", () => {
    expect(searchModels(CATALOG, "").map((h) => h.model.id)).toEqual(["opus", "sonnet", HAIKU.id, "gpt-6.1-sol", "gpt-6-luna"]);
    expect(searchModels(CATALOG, "fast").map((h) => h.model.name)).toEqual(["Haiku 4.5", "GPT-6 Luna"]);
    expect(searchModels(CATALOG, "codex sol").map((h) => h.model.name)).toEqual(["GPT-6.1 Sol"]);
    expect(searchModels(CATALOG, "opus 5.5").map((h) => h.agent)).toEqual(["claude"]);
    expect(searchModels(null, "opus")).toEqual([]);
  });
});
