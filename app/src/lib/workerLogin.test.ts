import { describe, expect, test } from "bun:test";
import type { AgentUsage, TransferInfo } from "@chauffeur/protocol";
import { afterLoginEnded, loginKey, loginResumes, signedOutAgents, startedLogin, type WorkerLogin } from "./workerLogin";

const refused: TransferInfo = {
  id: "t1",
  workspace_id: "w",
  slug: "moving",
  source: "mac",
  destination: "w1",
  phase: "failed",
  log: [],
  error: "w1 cannot take moving yet",
  workspace_path: null,
  kept_copy: null,
  returned: false,
  keep_local_copy: false,
  started_at: "2026-10-29T10:00:00Z",
  finished_at: null,
  working_agents: 0,
  interrupt: false,
  archive: false,
  needs_login: { worker: "w1", agents: ["claude", "codex"] },
};

const usage = (agent: AgentUsage["agent"], account: AgentUsage["account"]): AgentUsage => ({
  agent,
  plan: null,
  windows: [],
  error: null,
  read_at: null,
  failed_at: null,
  extra_usage: null,
  account,
  account_error: null,
});

describe("worker logins", () => {
  test("Claude's page asks for its code back; Codex's gives one to type there", () => {
    expect(startedLogin({ url: "https://claude.com/manual", asks_code: true })).toEqual({
      step: "browser",
      url: "https://claude.com/manual",
      userCode: null,
      asksCode: true,
      sending: false,
    });
    expect(startedLogin({ url: "https://auth.openai.com/codex/device", user_code: "ABCD-1234" })).toMatchObject({
      userCode: "ABCD-1234",
      asksCode: false,
    });
  });

  test("the end of a worker login followed here settles it, once", () => {
    const logins: Record<string, WorkerLogin> = { [loginKey("w1", "claude")]: { step: "starting" } };
    const done = afterLoginEnded(logins, { type: "login_ended", agent: "claude", worker: "w1", success: true, error: null });
    expect(done).toEqual({ [loginKey("w1", "claude")]: { step: "done" } });
    expect(afterLoginEnded(done!, { type: "login_ended", agent: "claude", worker: "w1", success: false, error: "late" })).toBeNull();
  });

  test("this machine's own login, or one on a worker not followed here, changes nothing", () => {
    const logins: Record<string, WorkerLogin> = { [loginKey("w1", "claude")]: { step: "starting" } };
    expect(afterLoginEnded(logins, { type: "login_ended", agent: "claude", success: true, error: null })).toBeNull();
    expect(afterLoginEnded(logins, { type: "login_ended", agent: "codex", worker: "w1", success: true, error: null })).toBeNull();
  });

  test("a failed login says why", () => {
    const logins: Record<string, WorkerLogin> = { [loginKey("w1", "codex")]: { step: "starting" } };
    expect(afterLoginEnded(logins, { type: "login_ended", agent: "codex", worker: "w1", success: false, error: null })).toEqual({
      [loginKey("w1", "codex")]: { step: "failed", error: "the login did not complete" },
    });
  });

  test("a refused transfer goes again once one of its agents is signed in", () => {
    expect(loginResumes(refused, {})).toBe(false);
    expect(loginResumes(refused, { [loginKey("w1", "codex")]: { step: "starting" } })).toBe(false);
    expect(loginResumes(refused, { [loginKey("w1", "codex")]: { step: "done" } })).toBe(true);
    expect(loginResumes(refused, { [loginKey("w2", "codex")]: { step: "done" } })).toBe(false);
    expect(loginResumes({ ...refused, phase: "running" }, { [loginKey("w1", "codex")]: { step: "done" } })).toBe(false);
  });

  test("only an agent read as signed out is offered a sign-in", () => {
    expect(signedOutAgents([usage("claude", "signed_out"), usage("codex", "unknown")])).toEqual(["claude"]);
  });
});
