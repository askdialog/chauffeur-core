// Local deterministic inference for real agent runtimes running with synthetic credentials.
// Assertions inspect the model request, not text guessed by a model.
import assert from "node:assert/strict";
import { toolEvidence, type FixtureTool } from "./tools.ts";

export function fixtureProvider(answer: string, tools: FixtureTool[] = [], trace?: (body: Record<string, unknown>) => void) {
  const requests: unknown[] = [];
  const compactRequests: unknown[] = [];
  let summary: string | undefined;
  let step = 0;
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port: 0,
    async fetch(request) {
      const path = new URL(request.url).pathname;
      if (request.method !== "POST") return new Response("fixture endpoint", { status: 404 });
      const body = await request.json() as Record<string, unknown>;
      trace?.(body);
      if (path.endsWith("/count_tokens")) return Response.json({ input_tokens: 10 });
      if (path.endsWith("/messages")) {
        (summary ? compactRequests : requests).push(body.messages);
        checkPreviousTool("claude", body.messages);
        const tool = tools[step++];
        const content = tool ? [{ type: "tool_use", id: tool.id, name: tool.name, input: tool.input }] : [{ type: "text", text: summary ?? answer }];
        const message = {
          id: `msg_${crypto.randomUUID()}`, type: "message", role: "assistant", model: body.model,
          content, stop_reason: tool ? "tool_use" : "end_turn", stop_sequence: null,
          usage: { input_tokens: 10, output_tokens: 10 },
        };
        if (!body.stream) return Response.json(message);
        return sse([
          { type: "message_start", message: { ...message, content: [], stop_reason: null } },
          { type: "content_block_start", index: 0, content_block: tool ? { type: "tool_use", id: tool.id, name: tool.name, input: {} } : { type: "text", text: "" } },
          { type: "content_block_delta", index: 0, delta: tool ? { type: "input_json_delta", partial_json: JSON.stringify(tool.input) } : { type: "text_delta", text: summary ?? answer } },
          { type: "content_block_stop", index: 0 },
          { type: "message_delta", delta: { stop_reason: message.stop_reason, stop_sequence: null }, usage: { output_tokens: 10 } },
          { type: "message_stop" },
        ]);
      }
      if (path.endsWith("/responses")) {
        (summary ? compactRequests : requests).push(body.input);
        checkPreviousTool("codex", body.input);
        const tool = tools[step];
        if (tool) {
          const input = body.input as Array<{ type: string; tools?: Array<{ name: string; tools?: Array<{ name: string }> }> }>;
          const advertised = input.filter((entry) => entry.type === "additional_tools").flatMap((entry) => entry.tools ?? []);
          assert(advertised.some((entry) => entry.name === "functions" && entry.tools?.some((nested) => nested.name === tool.name)), "Codex did not advertise functions.exec");
        }
        step++;
        const item = tool
          ? { id: `ctc_${crypto.randomUUID()}`, type: "custom_tool_call", namespace: "functions", call_id: tool.id, name: tool.name, input: tool.input.code, status: "completed" }
          : { id: `msg_${crypto.randomUUID()}`, type: "message", role: "assistant", status: "completed",
            content: [{ type: "output_text", text: summary ?? answer, annotations: [] }] };
        const response = { id: `resp_${crypto.randomUUID()}`, object: "response", created_at: Math.floor(Date.now() / 1000),
          status: "completed", model: body.model, output: [item],
          usage: { input_tokens: 10, output_tokens: 10, total_tokens: 20, input_tokens_details: { cached_tokens: 0 } } };
        return sse([
          { type: "response.created", response: { ...response, status: "in_progress", output: [] } },
          { type: "response.output_item.added", output_index: 0, item: tool ? { ...item, status: "in_progress", input: "" } : { ...item, status: "in_progress", content: [] } },
          ...(tool ? [
            { type: "response.custom_tool_call_input.delta", item_id: item.id, output_index: 0, delta: tool.input.code },
            { type: "response.custom_tool_call_input.done", item_id: item.id, output_index: 0, input: tool.input.code },
          ] : [
            { type: "response.content_part.added", item_id: item.id, output_index: 0, content_index: 0, part: { type: "output_text", text: "", annotations: [] } },
            { type: "response.output_text.delta", item_id: item.id, output_index: 0, content_index: 0, delta: summary ?? answer },
            { type: "response.output_text.done", item_id: item.id, output_index: 0, content_index: 0, text: summary ?? answer },
          ]),
          { type: "response.output_item.done", output_index: 0, item },
          { type: "response.completed", response },
        ]);
      }
      return new Response("unexpected fixture endpoint", { status: 404 });
    },
  });
  return { url: `http://127.0.0.1:${server.port}`, requests, compactRequests,
    setCompactionSummary: (value: string | undefined) => { summary = value; }, close: () => server.stop(true) };

  function checkPreviousTool(agent: string, history: unknown): void {
    const previous = tools[step - 1];
    if (!previous) return;
    const result = toolEvidence(agent, history).find((entry) => entry.id === previous.id);
    assert(result, "runtime did not execute the requested tool");
    assert.deepEqual(result.input, previous.input, "native runtime changed tool arguments");
    for (const line of previous.expectedRead?.trimEnd().split("\n") ?? []) {
      assert(JSON.stringify(result.output).includes(line), "native read did not return restored file contents");
    }
  }
}

function sse(events: Array<{ type: string; [key: string]: unknown }>): Response {
  return new Response(events.map((event) => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(""), {
    headers: { "content-type": "text/event-stream" },
  });
}
