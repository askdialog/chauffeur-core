import { expect, test } from "bun:test";
import { accountMetadata as claude, readNativeAccount as readClaude } from "../src/agents/claude/account.ts";
import { accountMetadata as codex, readNativeAccount as readCodex } from "../src/agents/codex/account.ts";
import { FakeAppServer, HOOK } from "./support/codex-fakes.ts";
import { FakeQuery } from "./support/fakes.ts";
import { silentLogger } from "../src/log.ts";

test("Claude mode is conservative when native sources overlap or are unknown", () => {
  const account = { email: "dev@example.com", organization: "Test", apiProvider: "firstParty" as const, subscriptionType: "max" };
  expect(claude(account)).toEqual({ mode: "subscription", email: "dev@example.com", organization: "Test" });
  expect(claude({ ...account, apiKeySource: "env" }).mode).toBe("api_key");
  expect(claude({ ...account, apiProvider: "bedrock" }).mode).toBe("external");
  expect(claude({ ...account, tokenSource: "unrecognized" }).mode).toBe("unknown");
  expect(claude({ ...account, tokenSource: "CLAUDE_CODE_OAUTH_TOKEN" }).mode).toBe("unknown");
  expect(claude({ ...account, tokenSource: "claude.ai" }).mode).toBe("subscription");
  expect(claude({ email: "dev@example.com" }).mode).toBe("unknown");
  expect(claude({ tokenSource: "none", apiKeySource: "none" }).mode).toBe("signed_out");
  expect(claude({ ...account, email: "bad\nvalue", organization: "x".repeat(257) })).toEqual({ mode: "subscription", email: null, organization: null });
  expect(claude({ ...account, subscriptionType: "max", extra: "private-secret" } as never)).not.toHaveProperty("extra");
});

test("Codex exposes only metadata and distinguishes native subscription, API, external and signed out", () => {
  expect(codex({ type: "chatgpt", email: "dev@example.com", planType: "pro", workspaceName: null })).toEqual({ mode: "subscription", email: "dev@example.com", organization: null });
  expect(codex({ type: "apiKey" }).mode).toBe("api_key");
  expect(codex({ type: "amazonBedrock", usesCodexManagedCredentials: false }).mode).toBe("external");
  expect(codex(null).mode).toBe("signed_out");
  expect(codex({ type: "chatgpt", email: " ", planType: "pro", workspaceName: null }).email).toBeNull();
});

test("Codex inspection does not refresh tokens, start a thread or request usage and always closes", async () => {
  const made: FakeAppServer[] = [];
  const deps = {
    spawn: (options: ConstructorParameters<typeof FakeAppServer>[0]) => { const server = new FakeAppServer(options); made.push(server); return server; },
    resolveExecutable: () => "/usr/local/bin/codex", baseEnv: { PATH: "/usr/bin" }, hookCommand: () => HOOK,
  };
  expect((await readCodex(deps, silentLogger)).mode).toBe("subscription");
  expect(made[0]!.requests.map(r => r.method)).toEqual(["initialize", "account/read"]);
  expect(made[0]!.sent("account/read")).toEqual([{ refreshToken: false }]);
  expect(made[0]!.closed).toBe(true);
  deps.spawn = options => { const server = new FakeAppServer(options); server.handlers["account/read"] = () => { throw new Error("private-token"); }; made.push(server); return server; };
  await expect(readCodex(deps, silentLogger)).rejects.toThrow("Could not inspect the native Codex account; check its local login");
  expect(made[1]!.closed).toBe(true);
});

test("Claude inspection sends no prompt, uses a nonpersistent idle session and aborts after reading", async () => {
  let signal: AbortSignal | undefined;
  let calls = 0;
  const deps = {
    query: ({ prompt, options }: any) => {
      expect(options.tools).toEqual([]); expect(options.settingSources).toEqual([]); expect(options.persistSession).toBe(false);
      signal = options.abortController.signal;
      const query = new FakeQuery(prompt, options);
      return Object.assign(query, { accountInfo: async () => { calls++; return { email: "dev@example.com", subscriptionType: "max" }; } });
    }, resolveExecutable: () => undefined, baseEnv: {},
  } as never;
  expect((await readClaude(deps)).email).toBe("dev@example.com");
  expect(calls).toBe(1); expect(signal!.aborted).toBe(true);
  await expect(readClaude({ ...deps as object, query: () => { throw new Error("private-token"); } } as never)).rejects.toThrow("Could not inspect the native Claude Code account; check its local login");
});


test("Claude closes the idle CLI and sanitizes account-read failures", async () => {
  let signal: AbortSignal | undefined;
  const deps = {
    query: ({ prompt, options }: any) => {
      signal = options.abortController.signal;
      const query = new FakeQuery(prompt, options);
      return Object.assign(query, { accountInfo: async () => { throw new Error("private-native-credential"); } });
    }, resolveExecutable: () => undefined, baseEnv: {},
  } as never;
  await expect(readClaude(deps)).rejects.toThrow("Could not inspect the native Claude Code account; check its local login");
  expect(signal!.aborted).toBe(true);
});
