import { describe, expect, test } from "bun:test";
import { heldBackDelegate } from "../src/agents/codex/repo-tools.ts";

const call = (tool = "delegate", turnId = "t1") => ({
  threadId: "th",
  turnId,
  callId: "c2",
  namespace: "chauffeur",
  tool,
  arguments: { prompt: "Implement the home screens" },
});

describe("a delegate call Codex held back", () => {
  const longCall = { turnId: "t1", startedAt: 0, endedAt: 18 * 60_000 };

  test("is refused when it starts the instant a long call of its turn answered", () => {
    const refused = heldBackDelegate(call(), longCall, longCall.endedAt + 2);
    expect(refused?.error).toBe(true);
    expect(refused?.text).toContain("about 18 min after you made it");
  });

  test("starts when the model made it after reading the previous result", () => {
    expect(heldBackDelegate(call(), longCall, longCall.endedAt + 4_000)).toBeNull();
  });

  test("starts after a short call, in another turn, or for another tool", () => {
    expect(heldBackDelegate(call(), { turnId: "t1", startedAt: 0, endedAt: 2_000 }, 2_002)).toBeNull();
    expect(heldBackDelegate(call("delegate", "t2"), longCall, longCall.endedAt + 2)).toBeNull();
    expect(heldBackDelegate(call("list_repos"), longCall, longCall.endedAt + 2)).toBeNull();
    expect(heldBackDelegate(call(), undefined, 5)).toBeNull();
  });
});
