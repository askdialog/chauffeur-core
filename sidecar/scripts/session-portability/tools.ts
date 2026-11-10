import assert from "node:assert/strict";

export type FixtureTool = { id: string; name: string; input: Record<string, unknown>; expectedRead?: string };
export type ToolEvidence = { id: string; name: string; namespace?: string; input: unknown; output: unknown };

/** Read native model inputs, not sidecar UI events or fabricated tool outputs. */
export function toolEvidence(agent: string, history: unknown): ToolEvidence[] {
  assert(Array.isArray(history), "expected native history array");
  const calls = new Map<string, { name: string; namespace?: string; input: unknown }>();
  const results: ToolEvidence[] = [];
  for (const item of history) {
    if (agent === "claude") {
      if (!Array.isArray(item.content)) continue;
      for (const block of item.content) {
        if (block.type === "tool_use") calls.set(block.id, { name: block.name, input: block.input });
        if (block.type === "tool_result") {
          const call = calls.get(block.tool_use_id);
          assert(call, "orphaned Claude tool result");
          assert(!block.is_error, "native Claude tool failed");
          results.push({ id: block.tool_use_id, ...call, output: block.content });
        }
      }
    } else {
      if (item.type === "function_call") calls.set(item.call_id, { name: item.name, input: JSON.parse(item.arguments) });
      if (item.type === "custom_tool_call") calls.set(item.call_id, { name: item.name,
        ...(item.namespace ? { namespace: item.namespace } : {}), input: { code: item.input } });
      if (item.type === "function_call_output" || item.type === "custom_tool_call_output") {
        const call = calls.get(item.call_id);
        assert(call, "orphaned Codex tool result");
        results.push({ id: item.call_id, ...call, output: item.output });
      }
    }
  }
  return results;
}

/** Commands use generated fixture data only, with POSIX quoting even if that data evolves. */
export function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}
