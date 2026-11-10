import { describe, expect, test } from "bun:test";
import { claudeLogin, claudeLoginCancel, claudeLoginCode } from "../src/agents/claude/login.ts";
import { claudeUsage } from "../src/agents/claude/usage.ts";
import { codexLogin, codexLoginCancel } from "../src/agents/codex/login.ts";
import { readCodexUsage } from "../src/agents/codex/usage.ts";
import { silentLogger } from "../src/log.ts";
import { FakeAppServer, HOOK } from "./support/codex-fakes.ts";
import { FakeQuery, flush } from "./support/fakes.ts";

function codexDeps(prepare: (server: FakeAppServer) => void = () => {}) {
  const made: FakeAppServer[] = [];
  return {
    made,
    deps: {
      spawn: (options: ConstructorParameters<typeof FakeAppServer>[0]) => {
        const server = new FakeAppServer(options);
        prepare(server);
        made.push(server);
        return server;
      },
      resolveExecutable: () => "/usr/local/bin/codex",
      baseEnv: { PATH: "/usr/bin" },
      hookCommand: () => HOOK,
    },
  };
}

/** An idle Claude CLI as the SDK gives it, with the account and login answers set by the test. */
class LoginQuery extends FakeQuery {
  account: Record<string, unknown> = { email: "dev@example.com", apiProvider: "firstParty" };
  completion: Promise<unknown> = new Promise(() => {});
  async accountInfo() {
    return this.account;
  }
  async claudeAuthenticate(loginWithClaudeAi: boolean) {
    this.calls.push(`claudeAuthenticate:${loginWithClaudeAi}`);
    return { manualUrl: "https://claude.ai/oauth/manual", automaticUrl: "https://claude.ai/oauth/auto" };
  }
  claudeOAuthWaitForCompletion() {
    return this.completion;
  }
  /** Settles the flow as the CLI would with the code: a wrong one fails it. */
  settle: (code: string, state: string | undefined) => Promise<unknown> = async () => {};
  claudeOAuthCallback(code: string, state?: string) {
    this.calls.push(`claudeOAuthCallback:${code}:${state}`);
    return this.settle(code, state);
  }
}

function claudeDeps(prepare: (query: LoginQuery) => void) {
  return {
    query: ({ prompt, options }: { prompt: AsyncIterable<never>; options: never }) => {
      const query = new LoginQuery(prompt, options);
      prepare(query);
      return query;
    },
    resolveExecutable: () => undefined,
    baseEnv: {},
  } as never;
}

describe("agent accounts", () => {
  test("Claude logged in to nothing reads as signed out, without asking its usage", async () => {
    const usage = await claudeUsage(
      claudeDeps((query) => {
        query.account = { tokenSource: "none", apiProvider: "firstParty" };
      }),
    );
    expect(usage).toEqual({ plan: null, windows: [], extra_usage: null, signed_out: "Claude Code is not logged in" });
  });

  test("Codex logged out reads as signed out", async () => {
    const { deps } = codexDeps((server) => {
      server.account = { account: null, requiresOpenaiAuth: true, workspaceRouting: null };
    });
    expect(await readCodexUsage(deps, silentLogger)).toEqual({ plan: null, windows: [], extra_usage: null, signed_out: "Codex is not logged in" });
  });

  test("Claude's login gives the page that comes back to the CLI, and says when it completed", async () => {
    let finish!: () => void;
    let query: LoginQuery | undefined;
    const ended: Array<[boolean, string | null]> = [];
    const result = await claudeLogin(
      claudeDeps((q) => {
        query = q;
        q.completion = new Promise<void>((resolve) => (finish = resolve));
      }),
      (success, error) => ended.push([success, error]),
    );
    expect(result).toEqual({ url: "https://claude.ai/oauth/auto" });
    expect(query?.calls).toContain("claudeAuthenticate:true");
    expect(ended).toEqual([]);
    finish();
    await flush();
    expect(ended).toEqual([[true, null]]);
  });

  test("Claude's login gives up after its timeout", async () => {
    const ended: Array<[boolean, string | null]> = [];
    await claudeLogin(
      claudeDeps(() => {}),
      (success, error) => ended.push([success, error]),
      false,
      10,
    );
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(ended).toEqual([[false, "the login was not completed in time"]]);
  });

  test("Claude's headless login gives the manual page and completes with the code pasted back", async () => {
    let finish!: () => void;
    let query: LoginQuery | undefined;
    const ended: Array<[boolean, string | null]> = [];
    const result = await claudeLogin(
      claudeDeps((q) => {
        query = q;
        q.completion = new Promise<void>((resolve) => (finish = resolve));
        q.settle = async () => finish();
      }),
      (success, error) => ended.push([success, error]),
      true,
    );
    expect(result).toEqual({ url: "https://claude.ai/oauth/manual", asks_code: true });
    await claudeLoginCode(" abc123#state-9\n");
    expect(query?.calls).toContain("claudeOAuthCallback:abc123:state-9");
    await flush();
    expect(ended).toEqual([[true, null]]);
    expect(() => claudeLoginCode("again#x")).toThrow("No Claude Code login waits for a code");
  });

  test("a code Claude refuses fails the call and ends the login", async () => {
    let fail!: (error: Error) => void;
    const ended: Array<[boolean, string | null]> = [];
    await claudeLogin(
      claudeDeps((q) => {
        q.completion = new Promise<void>((_, reject) => (fail = reject));
        q.settle = async () => {
          const error = new Error("Invalid code");
          fail(error);
          throw error;
        };
      }),
      (success, error) => ended.push([success, error]),
      true,
    );
    await expect(claudeLoginCode("wrong#s")).rejects.toThrow("Claude Code refused the code: Invalid code");
    await flush();
    expect(ended).toEqual([[false, "Invalid code"]]);
  });

  test("a login replaced by a newer one is not reported; a cancelled one is", async () => {
    const ended: Array<[boolean, string | null]> = [];
    const deps = claudeDeps(() => {});
    await claudeLogin(deps, (success, error) => ended.push([success, error]), true);
    await claudeLogin(deps, (success, error) => ended.push([success, error]), true);
    expect(ended).toEqual([]);
    claudeLoginCancel();
    expect(ended).toEqual([[false, "the login was cancelled"]]);
  });

  test("Codex's headless login gives the device code page and its code", async () => {
    const { deps, made } = codexDeps();
    const ended: Array<[boolean, string | null]> = [];
    const result = await codexLogin(deps, silentLogger, (success, error) => ended.push([success, error]), true);
    expect(result).toEqual({ url: "https://auth.openai.com/codex/device", user_code: "ABCD-1234" });
    const [server] = made;
    expect(server?.sent("account/login/start")).toEqual([{ type: "chatgptDeviceCode" }]);
    server?.push({ method: "account/login/completed", params: { loginId: "device-1", success: true, error: null, onboardingEntrypoint: null } });
    await flush();
    expect(ended).toEqual([[true, null]]);
  });

  test("Codex's device code login that times out says how to turn device code sign-in on", async () => {
    const { deps } = codexDeps();
    const ended: Array<[boolean, string | null]> = [];
    await codexLogin(deps, silentLogger, (success, error) => ended.push([success, error]), true, 10);
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(ended).toHaveLength(1);
    expect(ended[0]?.[0]).toBe(false);
    expect(ended[0]?.[1]).toContain("turn device code sign-in on in ChatGPT → Settings → Security");
  });

  test("cancelling Codex's login cancels it in the app-server and reports it", async () => {
    const { deps, made } = codexDeps();
    const ended: Array<[boolean, string | null]> = [];
    await codexLogin(deps, silentLogger, (success, error) => ended.push([success, error]), true);
    codexLoginCancel();
    expect(made[0]?.sent("account/login/cancel")).toEqual([{ loginId: "device-1" }]);
    expect(ended).toEqual([[false, "the login was cancelled"]]);
  });

  test("Codex's login gives ChatGPT's page, and says when it completed", async () => {
    const { deps, made } = codexDeps();
    const ended: Array<[boolean, string | null]> = [];
    const result = await codexLogin(deps, silentLogger, (success, error) => ended.push([success, error]));
    expect(result).toEqual({ url: "https://auth.openai.com/oauth/authorize?state=1" });
    const [server] = made;
    expect(server?.sent("account/login/start")).toEqual([{ type: "chatgpt" }]);
    server?.push({ method: "account/login/completed", params: { loginId: "login-1", success: true, error: null, onboardingEntrypoint: null } });
    await flush();
    expect(ended).toEqual([[true, null]]);
    expect(server?.closed).toBe(true);
  });
});
