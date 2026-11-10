import { describe, expect, test } from "bun:test";
import { leftoverLines, promoteLeftovers, promoteUnavailable } from "./promote";
import { RpcCallError } from "./rpc";

describe("promoteLeftovers", () => {
  const repos = [{ repo: "alpha", files: ["README.md", ".cache/"] }];

  test("reads the files of a promote refused for them", () => {
    const error = new RpcCallError("workspace.promote", { code: 409, message: "x", data: { repos } });
    expect(promoteLeftovers(error)).toEqual(repos);
  });

  test("any other refusal is an error to show", () => {
    expect(promoteLeftovers(new RpcCallError("workspace.promote", { code: 409, message: "an agent is working" }))).toBeNull();
    expect(promoteLeftovers(new RpcCallError("workspace.promote", { code: 400, message: "x", data: { repos } }))).toBeNull();
    expect(promoteLeftovers(new RpcCallError("workspace.promote", { code: 409, message: "x", data: { repos: [] } }))).toBeNull();
    expect(promoteLeftovers(new Error("x"))).toBeNull();
  });
});

test("leftoverLines shows the first files and counts the rest", () => {
  expect(leftoverLines(["a", "b"], 3)).toEqual({ shown: ["a", "b"], more: 0 });
  expect(leftoverLines(["a", "b", "c", "d"], 3)).toEqual({ shown: ["a", "b", "c"], more: 1 });
});

describe("promoteUnavailable", () => {
  test("a ready exploration whose agents are idle can continue", () => {
    expect(promoteUnavailable({ state: "ready", activity: "idle" })).toBeNull();
    expect(promoteUnavailable({ state: "ready", activity: "error" })).toBeNull();
  });

  test("an agent working or waiting for the user holds it back, as the daemon does", () => {
    expect(promoteUnavailable({ state: "ready", activity: "working" })).toContain("agent is working");
    expect(promoteUnavailable({ state: "ready", activity: "needs_input" })).toContain("agent is working");
  });

  test("an exploration not ready yet cannot continue", () => {
    expect(promoteUnavailable({ state: "creating", activity: "idle" })).toContain("ready");
  });
});
