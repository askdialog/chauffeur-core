import { describe, expect, test } from "bun:test";
import type { ModelInfo } from "@anthropic-ai/claude-agent-sdk";
import { claudeModels, listClaudeModels } from "../src/agents/claude/models.ts";
import { codexDisplayName, codexModels, listCodexModels } from "../src/agents/codex/models.ts";
import type { Model } from "../src/agents/codex/protocol.ts";
import { silentLogger } from "../src/log.ts";
import { FakeAppServer, HOOK } from "./support/codex-fakes.ts";
import { FakeQuery } from "./support/fakes.ts";

const OPUS: ModelInfo = {
  value: "opus",
  resolvedModel: "claude-opus-5-5",
  displayName: "Opus 5.5",
  description: "For complex work",
  supportsEffort: true,
  supportedEffortLevels: ["low", "medium", "high", "xhigh", "max"],
  supportsFastMode: true,
};

describe("Claude's catalog", () => {
  test("drops the Default row and marks the model it resolves to", () => {
    const rows: ModelInfo[] = [
      { value: "default", resolvedModel: "claude-opus-5-5", displayName: "Default (recommended)", description: "Opus 5.5" },
      OPUS,
      { value: "claude-haiku-4-5-20251001", resolvedModel: "claude-haiku-4-5-20251001", displayName: "Haiku 4.5", description: "Fastest" },
    ];
    expect(claudeModels(rows)).toEqual([
      {
        id: "opus",
        resolved: "claude-opus-5-5",
        name: "Opus 5.5",
        description: "For complex work",
        efforts: ["low", "medium", "high", "xhigh", "max"],
        default_effort: null,
        supports_fast: true,
        is_default: true,
      },
      {
        id: "claude-haiku-4-5-20251001",
        resolved: "claude-haiku-4-5-20251001",
        name: "Haiku 4.5",
        description: "Fastest",
        efforts: [],
        default_effort: null,
        supports_fast: false,
        is_default: false,
      },
    ]);
  });

  test("reads the catalog from a query that never gets a prompt, then closes it", async () => {
    let query: FakeQuery | undefined;
    const models = await listClaudeModels({
      query: ({ prompt, options }) => {
        query = new FakeQuery(prompt, options);
        query.models = [OPUS];
        return query;
      },
      resolveExecutable: () => "/opt/claude",
      baseEnv: { PATH: "/usr/bin" },
    });
    expect(models.map((m) => m.id)).toEqual(["opus"]);
    expect(query?.options).toMatchObject({ settingSources: ["user"], pathToClaudeCodeExecutable: "/opt/claude" });
    expect(query?.sent).toEqual([]);
    expect(query?.calls).toEqual(["supportedModels", "close"]);
  });
});

function codexModel(overrides: Partial<Model>): Model {
  return {
    id: "gpt-6.1-sol",
    model: "gpt-6.1-sol",
    upgrade: null,
    upgradeInfo: null,
    availabilityNux: null,
    displayName: "GPT-6.1-Sol",
    description: "Latest workhorse model",
    modelSpecialty: null,
    hidden: false,
    supportedReasoningEfforts: [
      { reasoningEffort: "low", description: "" },
      { reasoningEffort: "ultra", description: "" },
    ],
    defaultReasoningEffort: "low",
    inputModalities: [],
    supportsPersonality: false,
    multiAgentVersion: null,
    additionalSpeedTiers: [],
    serviceTiers: [{ id: "priority", name: "Fast", description: "2x speed" }],
    defaultServiceTier: null,
    availableAccessPrograms: null,
    isDefault: true,
    ...overrides,
  };
}

describe("Codex's catalog", () => {
  test("writes names as Codex's UI does", () => {
    expect(codexDisplayName("GPT-6.1-Sol")).toBe("GPT-6.1 Sol");
    expect(codexDisplayName("GPT-6-Astra")).toBe("GPT-6 Astra");
    expect(codexDisplayName("o4-mini")).toBe("o4-mini");
  });

  test("maps efforts, the default effort and the Fast tier; hides hidden models", () => {
    expect(
      codexModels([
        codexModel({}),
        codexModel({ model: "gpt-6-luna", displayName: "GPT-6-Luna", serviceTiers: [], isDefault: false }),
        codexModel({ model: "internal", hidden: true }),
      ]),
    ).toEqual([
      {
        id: "gpt-6.1-sol",
        resolved: "gpt-6.1-sol",
        name: "GPT-6.1 Sol",
        description: "Latest workhorse model",
        efforts: ["low", "ultra"],
        default_effort: "low",
        supports_fast: true,
        is_default: true,
      },
      {
        id: "gpt-6-luna",
        resolved: "gpt-6-luna",
        name: "GPT-6 Luna",
        description: "Latest workhorse model",
        efforts: ["low", "ultra"],
        default_effort: "low",
        supports_fast: false,
        is_default: false,
      },
    ]);
  });

  test("asks a short-lived app-server, without the guard hook, then closes it", async () => {
    let server: FakeAppServer | undefined;
    const models = await listCodexModels(
      {
        spawn: (options) => {
          server = new FakeAppServer(options);
          server.models = [codexModel({})];
          return server;
        },
        resolveExecutable: () => "/usr/local/bin/codex",
        baseEnv: { PATH: "/usr/bin" },
        hookCommand: () => HOOK,
      },
      silentLogger,
    );
    expect(models.map((m) => m.name)).toEqual(["GPT-6.1 Sol"]);
    expect(server?.options.args).toEqual([]);
    expect(server?.requests.map((r) => r.method)).toEqual(["initialize", "model/list"]);
    expect(server?.closed).toBe(true);
  });
});
