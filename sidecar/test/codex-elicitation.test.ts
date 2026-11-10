import { expect, test } from "bun:test";
import type { ChatEvent, JsonValue } from "../src/protocol.ts";
import { PendingRequests } from "../src/agents/pending.ts";
import { askMcpElicitation } from "../src/agents/codex/elicitation.ts";
import { CodexAlwaysAllowed } from "../src/agents/codex/always.ts";

function form(schema: JsonValue) {
  const events: ChatEvent[] = [];
  const requests = new PendingRequests((event) => events.push(event), () => "consent-1");
  const response = askMcpElicitation({ threadId: "thread", turnId: "turn", serverName: "computer-use", mode: "openai/form", _meta: null, message: "Allow Chrome?", requestedSchema: schema }, requests, new CodexAlwaysAllowed(), () => {});
  return { events, requests, response };
}

test("invalid answers leave consent pending; validated boolean and enum values retain their types", async () => {
  const { requests, events, response } = form({ type: "object", properties: {
    approved: { type: "boolean" }, scope: { type: "string", enum: ["once", "session"], enumNames: ["Once", "Session"] },
  }, required: ["approved", "scope"], additionalProperties: false });
  const invalid: JsonValue[] = [{ approved: "true", scope: "once" }, { approved: true, scope: "forever" }, { approved: true, scope: "once", extra: true }, {}];
  for (const content of invalid) {
    expect(() => requests.respond("consent-1", { type: "elicitation", action: "accept", content, always: false })).toThrow();
    expect(requests.size).toBe(1);
  }
  expect(() => requests.respond("consent-1", { type: "allow", always: true })).toThrow();
  expect(events.filter((event) => event.type === "request_resolved")).toHaveLength(0);
  requests.respond("consent-1", { type: "elicitation", action: "accept", content: { approved: false, scope: "once" }, always: false });
  expect(await response).toEqual({ action: "accept", content: { approved: false, scope: "once" }, _meta: null });
  expect(events.at(-1)).toEqual({ type: "request_resolved", request_id: "consent-1", outcome: "allowed" });
  expect(JSON.stringify(events)).not.toContain('"approved":false');
});

test.each(["decline", "cancel"] as const)("%s sends no form content", async (action) => {
  const { requests, response } = form({ type: "object", properties: {} });
  requests.respond("consent-1", { type: "elicitation", action, content: { password: "discard" }, always: false });
  expect(await response).toEqual({ action, content: null, _meta: null });
});

const unsupportedSchemas: JsonValue[] = [
  { type: "object", customApproval: true },
  { type: "object", $async: true },
  { type: "object", properties: { bad: { type: "invalid" } } },
  { type: "object", properties: { bad: { $ref: "https://example.com/schema" } } },
];
test.each(unsupportedSchemas)("unsupported schemas are visible and can only be declined or cancelled: %j", async (schema) => {
  const { requests, events, response } = form(schema);
  expect(events[0]).toMatchObject({ type: "mcp_elicitation", error: expect.stringContaining("Cannot accept") });
  expect(() => requests.respond("consent-1", { type: "elicitation", action: "accept", content: {}, always: false })).toThrow();
  requests.respond("consent-1", { type: "elicitation", action: "decline", content: null, always: false });
  expect(await response).toMatchObject({ action: "decline" });
});

test("2020-12 schemas enforce formats and numeric bounds without coercion", async () => {
  const { requests, response } = form({ $schema: "https://json-schema.org/draft/2020-12/schema", type: "object", properties: {
    email: { type: "string", format: "email" }, count: { type: "integer", minimum: 1, maximum: 3 },
  }, required: ["email", "count"] });
  for (const content of [{ email: "no", count: 1 }, { email: "test@example.com", count: 4 }, { email: "test@example.com", count: "1" }]) {
    expect(() => requests.respond("consent-1", { type: "elicitation", action: "accept", content, always: false })).toThrow();
  }
  requests.respond("consent-1", { type: "elicitation", action: "accept", content: { email: "test@example.com", count: 2 }, always: false });
  expect(await response).toMatchObject({ action: "accept" });
});

test.each(["https://example.com/confirm", "javascript:alert(1)", "https://user:password@example.com"])("URL consent is explicit, validated, and cancelled on interruption: %s", async (url) => {
  const events: ChatEvent[] = [];
  const requests = new PendingRequests((event) => events.push(event), () => "url-1");
  const response = askMcpElicitation({ mode: "url", threadId: "thread", turnId: null, serverName: "auth", _meta: null, message: "Confirm access", url, elicitationId: "native-1" }, requests, new CodexAlwaysAllowed(), () => {});
  expect(events[0]).toMatchObject({ type: "mcp_elicitation", url });
  if (url === "https://example.com/confirm") {
    expect(() => requests.respond("url-1", { type: "elicitation", action: "accept", content: {}, always: false })).toThrow();
    requests.respond("url-1", { type: "elicitation", action: "accept", content: null, always: false });
    expect(await response).toEqual({ action: "accept", content: null, _meta: null });
  } else {
    expect(() => requests.respond("url-1", { type: "elicitation", action: "accept", content: null, always: false })).toThrow();
    requests.cancelAll();
    expect(await response).toEqual({ action: "cancel", content: null, _meta: null });
  }
});

const CHROME = 'cua_repl: Allow Computer Use to use "Google Chrome"?';

function chrome(requests: PendingRequests, always: CodexAlwaysAllowed, schema: JsonValue = { type: "object", properties: {} }) {
  return askMcpElicitation({ threadId: "thread", turnId: "turn", serverName: "cua_repl", mode: "openai/form", _meta: null, message: 'Allow Computer Use to use "Google Chrome"?', requestedSchema: schema }, requests, always, () => {});
}

test("always accepting a confirmation with nothing to fill in accepts it, the open ones and the next ones in the chat", async () => {
  const events: ChatEvent[] = [];
  let id = 0;
  const requests = new PendingRequests((event) => events.push(event), () => `r${++id}`);
  const always = new CodexAlwaysAllowed();
  const first = chrome(requests, always);
  const second = chrome(requests, always);
  expect(events[0]).toMatchObject({ type: "mcp_elicitation", rule: CHROME });
  requests.respond("r1", { type: "elicitation", action: "accept", content: {}, always: true });
  expect(await first).toEqual({ action: "accept", content: {}, _meta: null });
  expect(await second).toEqual({ action: "accept", content: {}, _meta: null });
  expect(events.filter((event) => event.type === "request_resolved")).toEqual([
    { type: "request_resolved", request_id: "r1", outcome: "allowed", rules: [CHROME] },
    { type: "request_resolved", request_id: "r2", outcome: "allowed", rules: [CHROME] },
  ]);
  const count = events.length;
  expect(await chrome(requests, always)).toEqual({ action: "accept", content: {}, _meta: null });
  expect(events).toHaveLength(count);
  // Another app is asked again.
  void askMcpElicitation({ threadId: "thread", turnId: "turn", serverName: "cua_repl", mode: "openai/form", _meta: null, message: 'Allow Computer Use to use "Finder"?', requestedSchema: { type: "object", properties: {} } }, requests, always, () => {});
  expect(events.at(-1)).toMatchObject({ type: "mcp_elicitation", rule: 'cua_repl: Allow Computer Use to use "Finder"?' });
});

test("a session started with the chat's rules accepts the confirmation without asking", async () => {
  const events: ChatEvent[] = [];
  const requests = new PendingRequests((event) => events.push(event));
  expect(await chrome(requests, new CodexAlwaysAllowed([CHROME]))).toEqual({ action: "accept", content: {}, _meta: null });
  expect(events).toHaveLength(0);
});

test("a confirmation with a field to fill in offers no rule and cannot be always accepted", async () => {
  const events: ChatEvent[] = [];
  const requests = new PendingRequests((event) => events.push(event), () => "r1");
  const response = chrome(requests, new CodexAlwaysAllowed(), { type: "object", properties: { approved: { type: "boolean" } } });
  expect(events[0]).not.toHaveProperty("rule");
  expect(() => requests.respond("r1", { type: "elicitation", action: "accept", content: { approved: true }, always: true })).toThrow("answered each time");
  requests.respond("r1", { type: "elicitation", action: "accept", content: { approved: true }, always: false });
  expect(await response).toMatchObject({ action: "accept" });
  expect(events.at(-1)).toEqual({ type: "request_resolved", request_id: "r1", outcome: "allowed" });
});

test("always is refused on a declined confirmation", () => {
  const requests = new PendingRequests(() => {}, () => "r1");
  void chrome(requests, new CodexAlwaysAllowed());
  expect(() => requests.respond("r1", { type: "elicitation", action: "decline", content: null, always: true })).toThrow("answered each time");
  expect(requests.size).toBe(1);
});
