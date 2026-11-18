import { describe, expect, test } from "bun:test";
import { changeRequestTerm, forgeName, forgeWords, gitlabConnectingLabel } from "./forge";

describe("the words follow the forge", () => {
  test("GitHub, the default, says PR and pull request", () => {
    expect(changeRequestTerm("github")).toBe("PR");
    expect(changeRequestTerm(undefined, "long")).toBe("pull request");
    expect(forgeName(undefined)).toBe("GitHub");
  });

  test("GitLab says MR and merge request", () => {
    expect(changeRequestTerm("gitlab")).toBe("MR");
    expect(changeRequestTerm("gitlab", "long")).toBe("merge request");
    expect(forgeName("gitlab")).toBe("GitLab");
  });

  test("a GitLab connection reads as it goes", () => {
    const step = { phase: "downloading", completed: 5_000_000, total: 20_000_000, error: null };
    expect(gitlabConnectingLabel(step)).toStartWith("Getting GitLab's connector, 25%");
    expect(gitlabConnectingLabel({ ...step, phase: "signing_in" })).toBe("Signing in…");
    expect(gitlabConnectingLabel({ ...step, error: "bad token" })).toBe("Could not connect GitLab: bad token");
  });

  test("a change is numbered as its forge does", () => {
    expect(forgeWords("github").number(12)).toBe("#12");
    expect(forgeWords("gitlab").number(12)).toBe("!12");
    expect(forgeWords("gitlab").shorts).toBe("MRs");
    expect(forgeWords(undefined).long).toBe("pull request");
  });
});
