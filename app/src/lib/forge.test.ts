import { describe, expect, test } from "bun:test";
import { changeRequestTerm, forgeName, gitlabConnectingLabel } from "./forge";

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
});
