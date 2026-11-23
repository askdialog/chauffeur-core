import { describe, expect, test } from "bun:test";
import type { SlashCommand } from "@anthropic-ai/claude-agent-sdk";
import { claudeCommands, listClaudeCommands } from "../src/agents/claude/commands.ts";
import { FakeQuery } from "./support/fakes.ts";

const row = (name: string, over: Partial<SlashCommand> = {}): SlashCommand => ({
  name,
  description: `${name} it`,
  argumentHint: "",
  ...over,
});

describe("Claude's commands", () => {
  test("keeps skills, commands and the built-ins but those of the terminal or of a setting Chauffeur owns", () => {
    const rows = [
      row("architect", { argumentHint: "<feature>", description: "Design a feature (project)" }),
      row("acme-skills:implement-ticket"),
      row("code-review", { builtin: true, argumentHint: "[low|high]" }),
      row("model", { builtin: true }),
      row("clear", { builtin: true }),
      row("compact", { builtin: true }),
      row("color", { builtin: true }),
      row("focus", { builtin: true }),
      row("effort", { builtin: true }),
      row("fast", { builtin: true }),
    ];
    expect(claudeCommands(rows)).toEqual([
      { name: "architect", description: "Design a feature (project)", argument_hint: "<feature>", builtin: false },
      { name: "acme-skills:implement-ticket", description: "acme-skills:implement-ticket it", argument_hint: "", builtin: false },
      { name: "code-review", description: "code-review it", argument_hint: "[low|high]", builtin: true },
      { name: "clear", description: "clear it", argument_hint: "", builtin: true },
      { name: "compact", description: "compact it", argument_hint: "", builtin: true },
    ]);
  });

  test("a skill named like a built-in is the built-in, hidden with it", () => {
    const rows = [row("model"), row("simplify"), row("model", { builtin: true }), row("simplify", { builtin: true })];
    expect(claudeCommands(rows).map((c) => [c.name, c.builtin])).toEqual([["simplify", true]]);
  });

  test("a name listed twice appears once", () => {
    expect(claudeCommands([row("create-pull-request"), row("create-pull-request")])).toHaveLength(1);
  });

  test("asks a session-like query in the workspace with its repos, then closes it", async () => {
    let query: FakeQuery | undefined;
    const commands = await listClaudeCommands(
      {
        query: ({ prompt, options }) => {
          query = new FakeQuery(prompt, options);
          query.commands = [row("architect")];
          return query;
        },
        resolveExecutable: () => "/opt/claude",
        baseEnv: { PATH: "/usr/bin" },
      },
      "/ws",
      ["/ws/web", "/ws/api"],
    );
    expect(commands.map((c) => c.name)).toEqual(["architect"]);
    expect(query?.options).toMatchObject({
      cwd: "/ws",
      additionalDirectories: ["/ws/web", "/ws/api"],
      settingSources: ["user", "project", "local"],
      pathToClaudeCodeExecutable: "/opt/claude",
    });
    expect(query?.sent).toEqual([]);
    expect(query?.calls).toEqual(["supportedCommands", "close"]);
  });
});
