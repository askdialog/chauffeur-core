// Chauffeur's tools for an ACP agent: an MCP server of the sidecar's own, one per session, that the
// agent reaches over streamable HTTP on 127.0.0.1 with a bearer token of the session's. ACP's own
// MCP transport (`mcp/connect`) would spare the socket, but Vibe 3.1.3 refuses it ("unsupported ACP
// transport"). Each POST carries one JSON-RPC message and is answered with plain JSON, which the
// streamable HTTP transport allows; a call the agent gives up on closes its request, which aborts
// the tool.

import { randomBytes, timingSafeEqual } from "node:crypto";
import type { JsonValue } from "../../protocol.ts";
import { errorMessage, type Logger } from "../../log.ts";
import { SIDECAR_VERSION } from "../../version.ts";
import type { ToolText } from "../repo-tools.ts";
import type { McpServerSpec } from "./protocol.ts";

export interface ServedTool {
  name: string;
  description: string;
  inputSchema: JsonValue;
  run(input: unknown, signal: AbortSignal): Promise<ToolText>;
}

export interface ToolsServer {
  /** The server as a session's `mcpServers` entry. */
  readonly spec: McpServerSpec;
  close(): void;
}

const MCP_PROTOCOL_VERSION = "2025-06-18";

export function serveTools(name: string, instructions: string, tools: ServedTool[], log: Logger): ToolsServer {
  const token = randomBytes(32).toString("hex");
  const expected = Buffer.from(`Bearer ${token}`);
  const byName = new Map(tools.map((tool) => [tool.name, tool]));
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    // A tool may wait for the user or a delegated chat: no idle cut-off of Bun's.
    idleTimeout: 0,
    async fetch(request) {
      const given = Buffer.from(request.headers.get("authorization") ?? "");
      if (given.length !== expected.length || !timingSafeEqual(given, expected)) return new Response("unauthorized", { status: 401 });
      if (request.method === "DELETE") return new Response(null, { status: 200 });
      if (request.method !== "POST") return new Response(null, { status: 405 });
      let message: { id?: string | number; method?: string; params?: Record<string, unknown> };
      try {
        message = (await request.json()) as typeof message;
      } catch {
        return Response.json(rpcError(null, -32700, "not JSON"), { status: 400 });
      }
      // A notification or a response of the client's: nothing to answer.
      if (message.id === undefined || message.method === undefined) return new Response(null, { status: 202 });
      return Response.json(await answer(message.id, message.method, message.params ?? {}, request.signal));
    },
  });

  async function answer(id: string | number, method: string, params: Record<string, unknown>, signal: AbortSignal): Promise<object> {
    switch (method) {
      case "initialize":
        return {
          jsonrpc: "2.0",
          id,
          result: {
            protocolVersion: typeof params.protocolVersion === "string" ? params.protocolVersion : MCP_PROTOCOL_VERSION,
            capabilities: { tools: {} },
            serverInfo: { name, version: SIDECAR_VERSION },
            instructions,
          },
        };
      case "ping":
        return { jsonrpc: "2.0", id, result: {} };
      case "tools/list":
        return {
          jsonrpc: "2.0",
          id,
          result: { tools: tools.map((tool) => ({ name: tool.name, description: tool.description, inputSchema: tool.inputSchema })) },
        };
      case "tools/call": {
        const tool = byName.get(String(params.name));
        if (!tool) return { jsonrpc: "2.0", id, result: textResult({ text: `Chauffeur has no tool ${String(params.name)}.`, error: true }) };
        try {
          return { jsonrpc: "2.0", id, result: textResult(await tool.run(params.arguments ?? {}, signal)) };
        } catch (error) {
          log.warn(`${tool.name} failed: ${errorMessage(error)}`);
          return { jsonrpc: "2.0", id, result: textResult({ text: `${tool.name} failed: ${errorMessage(error)}`, error: true }) };
        }
      }
      default:
        return rpcError(id, -32601, `no method ${method}`);
    }
  }

  return {
    spec: {
      type: "http",
      name,
      url: `http://127.0.0.1:${server.port}/mcp`,
      headers: [{ name: "Authorization", value: `Bearer ${token}` }],
    },
    close: () => void server.stop(true),
  };
}

function textResult(said: ToolText): object {
  return { content: [{ type: "text", text: said.text }], isError: said.error };
}

function rpcError(id: string | number | null, code: number, message: string): object {
  return { jsonrpc: "2.0", id, error: { code, message } };
}
