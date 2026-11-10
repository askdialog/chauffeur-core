import { expect, test } from "bun:test";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { JsonRpcPeer } from "../src/rpc/peer.ts";
import { PendingRequests } from "../src/agents/pending.ts";
import { askMcpElicitation } from "../src/agents/codex/elicitation.ts";
import { CodexAlwaysAllowed } from "../src/agents/codex/always.ts";
import type { ChatEvent, RequestResponse } from "../src/protocol.ts";
import type { McpServerElicitationRequestParams } from "../src/agents/codex/generated/v2/McpServerElicitationRequestParams.ts";
import type { McpServerToolCallResponse } from "../src/agents/codex/generated/v2/McpServerToolCallResponse.ts";
import type { ThreadStartResponse } from "../src/agents/codex/generated/v2/ThreadStartResponse.ts";
import type { ListMcpServerStatusResponse } from "../src/agents/codex/generated/v2/ListMcpServerStatusResponse.ts";

// Opt-in: CHAUFFEUR_MCP_E2E=1 bun test test/codex-elicitation-native.test.ts
// Real app-server and local synthetic MCP server; no login, model turn or device access.
test.skipIf(process.env.CHAUFFEUR_MCP_E2E !== "1")("native Codex transports explicit MCP accept, decline and cancel without model turns", async () => {
  const root = mkdtempSync(join(tmpdir(), "chauffeur-native-consent-"));
  mkdirSync(join(root, "codex"));
  mkdirSync(join(root, "home"));
  writeFileSync(join(root, "home", "settings.toml"), "linear_enabled = false\n");
  const fixture = join(import.meta.dir, "fixtures", "mcp-consent.py");
  const proc = Bun.spawn(["codex", "app-server", "-c", `mcp_servers.consent_qa={command="python3",args=[${JSON.stringify(fixture)}]}`], {
    cwd: root, env: {
      ...process.env, CODEX_HOME: join(root, "codex"), CHAUFFEUR_HOME: join(root, "home"),
      CHAUFFEUR_WORKSPACES_DIR: join(root, "workspaces"), CHAUFFEUR_REPOS_DIR: join(root, "repos"),
      CHAUFFEUR_PORT_OVERRIDE: "0", CHAUFFEUR_WORKSPACE_ID: undefined,
    }, stdin: "pipe", stdout: "pipe", stderr: "pipe",
  });
  const stderr = new Response(proc.stderr).text();
  const deadline = setTimeout(() => proc.kill(), 25_000);
  const responses: Extract<RequestResponse, { type: "elicitation" }>[] = [
    { type: "elicitation", action: "accept", content: { approved: false }, always: false },
    { type: "elicitation", action: "decline", content: null, always: false },
    { type: "elicitation", action: "cancel", content: null, always: false },
  ];
  const events: ChatEvent[] = [];
  let next = 0;
  const requests = new PendingRequests((event) => {
    events.push(event);
    if (event.type === "mcp_elicitation") {
      const response = responses[next++];
      if (!response) throw new Error("unexpected extra confirmation");
      requests.respond(event.request_id, response);
    }
  });
  const peer = new JsonRpcPeer({
    write(line) { proc.stdin.write(line); void proc.stdin.flush(); },
    handlers: { onRequest(method, params) {
      if (method !== "mcpServer/elicitation/request") throw new Error(`unexpected ${method}`);
      const request: McpServerElicitationRequestParams = JSON.parse(JSON.stringify(params));
      return askMcpElicitation(request, requests, new CodexAlwaysAllowed(), () => {});
    } },
  });
  const reading = peer.run(proc.stdout);
  try {
    await peer.request("initialize", { clientInfo: { name: "chauffeur_qa", version: "1" }, capabilities: { experimentalApi: true } });
    peer.notify("initialized", {});
    const started: ThreadStartResponse = JSON.parse(JSON.stringify(await peer.request("thread/start", { cwd: root, approvalPolicy: "on-request", sandbox: "read-only" })));
    const threadId = started.thread.id;
    let ready = false;
    for (let attempt = 0; attempt < 50; attempt++) {
      const status: ListMcpServerStatusResponse = JSON.parse(JSON.stringify(await peer.request("mcpServerStatus/list", { threadId })));
      ready = status.data.some((server) => server.name === "consent_qa" && Object.keys(server.tools).length > 0);
      if (ready) break;
      await Bun.sleep(100);
    }
    expect(ready).toBe(true);
    for (const response of responses) {
      const result: McpServerToolCallResponse = JSON.parse(JSON.stringify(await peer.request("mcpServer/tool/call", { threadId, server: "consent_qa", tool: "confirm", arguments: {} })));
      const first = result.content[0];
      if (!first || typeof first !== "object" || Array.isArray(first) || typeof first.text !== "string") throw new Error("missing MCP response");
      expect(JSON.parse(first.text)).toEqual(response.action === "accept" ? { action: "accept", content: { approved: false } } : { action: response.action });
    }
    expect(events.filter((event) => event.type === "mcp_elicitation")).toHaveLength(3);
    expect(requests.size).toBe(0);
  } finally {
    requests.cancelAll();
    proc.stdin.end();
    await Promise.race([proc.exited, Bun.sleep(3_000)]);
    proc.kill();
    await reading;
    clearTimeout(deadline);
    await stderr;
    rmSync(root, { recursive: true, force: true });
  }
}, 30_000);
