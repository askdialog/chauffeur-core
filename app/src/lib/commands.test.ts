import { describe, expect, test } from "bun:test";
import type { SlashCommand } from "@chauffeur/protocol";
import { matchCommands, slashQuery } from "./commands";

const command = (name: string, description = ""): SlashCommand => ({ name, description, argument_hint: "", builtin: false });

describe("the / menu", () => {
  test("opens on a lone /word being typed", () => {
    expect(slashQuery("/")).toBe("");
    expect(slashQuery("/impl")).toBe("impl");
    expect(slashQuery("/acme-skills:impl")).toBe("acme-skills:impl");
    expect(slashQuery("/implement-ticket XYZ-1")).toBeNull();
    expect(slashQuery("fix /tmp")).toBeNull();
    expect(slashQuery("")).toBeNull();
  });

  test("ranks names starting with the query, then name parts, then the rest", () => {
    const commands = [
      command("review-standards", "Review a branch against CODING_STANDARDS"),
      command("acme-skills:implement-ticket"),
      command("simplify", "Review the changed code"),
      command("implement"),
      command("architect"),
    ];
    expect(matchCommands(commands, "impl").map((c) => c.name)).toEqual(["implement", "acme-skills:implement-ticket", "simplify"]);
    expect(matchCommands(commands, "REVIEW").map((c) => c.name)).toEqual(["review-standards", "simplify"]);
    expect(matchCommands(commands, "")).toEqual(commands);
    expect(matchCommands(commands, "zzz")).toEqual([]);
  });
});
