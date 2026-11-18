import { expect, test } from "bun:test";
import { serveTools } from "../src/agents/acp/tools-server.ts";
import { vibeModels, vibeUsage } from "../src/agents/vibe/account.ts";
import { resolveVibeExecutable } from "../src/agents/vibe/executable.ts";
import { vibeChauffeurTools } from "../src/agents/vibe/tools.ts";
import { PendingRequests } from "../src/agents/pending.ts";
import { silentLogger } from "../src/log.ts";
import type { ChatEvent } from "../src/protocol.ts";
import { CONFIG_OPTIONS } from "./support/acp-fakes.ts";
import { FakeHost, flush } from "./support/fakes.ts";

function tools(host: FakeHost, followUps: string[], options: { delegate?: boolean } = {}) {
  const events: ChatEvent[] = [];
  let ids = 0;
  const requests = new PendingRequests((event) => events.push(event), () => `req-${++ids}`);
  const served = vibeChauffeurTools(host, requests, {
    delegate: options.delegate ?? true,
    interrupted: () => new AbortController().signal,
    followUp: (text) => followUps.push(text),
    answerWithinMs: 20,
  });
  return { served, requests, events };
}

test("add_repo answers within its limit, and the user's later answer comes as a message", async () => {
  const host = new FakeHost();
  const followUps: string[] = [];
  const { served, requests } = tools(host, followUps);
  const addRepo = served.find((tool) => tool.name === "add_repo");
  const said = await addRepo?.run({ repo: "analytics", reason: "Dashboards" }, new AbortController().signal);
  expect(said?.text).toContain("has not answered yet");
  requests.respond("req-1", { type: "allow", always: false });
  await flush();
  expect(host.added).toEqual(["analytics"]);
  expect(followUps[0]).toContain("Chauffeur: the user answered your add_repo call. The user said yes: analytics");
});

test("delegate leaves a delegated chat that takes too long to work on, its answer to come as a message", async () => {
  const host = new FakeHost();
  const { served } = tools(host, []);
  const said = await served.find((tool) => tool.name === "delegate")?.run({ prompt: "Review the diff" }, new AbortController().signal);
  expect(said?.text).toContain("works on: its answer will come to you as a new message");
  expect(host.detached).toEqual(["sub-1"]);
});

test("a delegated chat has no delegate tool", () => {
  const { served } = tools(new FakeHost(), [], { delegate: false });
  expect(served.map((tool) => tool.name)).toEqual(["list_repos", "add_repo"]);
});

test("the tools server answers MCP over HTTP with its token only", async () => {
  const server = serveTools(
    "chauffeur",
    "Chauffeur's tools",
    [{ name: "echo", description: "Echoes", inputSchema: { type: "object" }, run: async (input) => ({ text: JSON.stringify(input), error: false }) }],
    silentLogger,
  );
  const spec = server.spec;
  if (!("url" in spec)) throw new Error("not an HTTP server");
  const [header] = spec.headers;
  const post = (body: object, auth = `${header?.value}`) =>
    fetch(spec.url, { method: "POST", headers: { authorization: auth, "content-type": "application/json" }, body: JSON.stringify(body) });
  expect((await post({ jsonrpc: "2.0", id: 1, method: "tools/list" }, "Bearer nope")).status).toBe(401);
  const listed = (await (await post({ jsonrpc: "2.0", id: 1, method: "tools/list" })).json()) as { result: { tools: { name: string }[] } };
  expect(listed.result.tools.map((tool) => tool.name)).toEqual(["echo"]);
  const called = (await (await post({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "echo", arguments: { a: 1 } } })).json()) as {
    result: { content: { type: string; text: string }[]; isError: boolean };
  };
  expect(called.result).toEqual({ content: [{ type: "text", text: '{"a":1}' }], isError: false });
  expect((await post({ jsonrpc: "2.0", method: "notifications/initialized" })).status).toBe(202);
  server.close();
});

test("the models come from the session's options, the usage from the plan's monthly budget", () => {
  expect(vibeModels(CONFIG_OPTIONS)).toEqual([
    { id: "mistral-large-4-0", resolved: "mistral-large-4-0", name: "Mistral Large 4", description: "", efforts: ["low", "medium", "high"], default_effort: "high", supports_fast: false, is_default: false },
    { id: "mistral-medium-latest", resolved: "mistral-medium-latest", name: "Mistral Medium", description: "", efforts: ["low", "medium", "high"], default_effort: "high", supports_fast: false, is_default: true },
  ]);
  expect(vibeUsage({ plan: { kind: "chat", name: "INDIVIDUAL", title: "Pro" }, budget: { usagePercentage: 12.5, resetAt: "2026-12-01T00:00:00Z", paygEnabled: false } })).toEqual({
    plan: "pro",
    windows: [{ label: "month", used_percent: 12.5, resets_at: "2026-12-01T00:00:00Z", window_secs: 2592000 }],
    extra_usage: null,
    signed_out: null,
  });
  expect(vibeUsage({ plan: null, budget: { usagePercentage: 99, paygEnabled: true } }).windows[0]?.label).toBe("month (pay-as-you-go)");
});

test("vibe-acp comes from CHAUFFEUR_VIBE_PATH, the PATH, or uv's tool folder", () => {
  const lookup = (found: string | null, exists: string[]) => ({ which: () => found, exists: (path: string) => exists.includes(path), home: "/home/me" });
  expect(resolveVibeExecutable({ CHAUFFEUR_VIBE_PATH: "/opt/vibe-acp" }, lookup(null, ["/opt/vibe-acp"]))).toBe("/opt/vibe-acp");
  expect(resolveVibeExecutable({ PATH: "/usr/bin" }, lookup("/usr/bin/vibe-acp", []))).toBe("/usr/bin/vibe-acp");
  expect(resolveVibeExecutable({ PATH: "/usr/bin" }, lookup(null, ["/home/me/.local/bin/vibe-acp"]))).toBe("/home/me/.local/bin/vibe-acp");
  expect(() => resolveVibeExecutable({ PATH: "/usr/bin" }, lookup(null, []))).toThrow("uv tool install mistral-vibe");
});
