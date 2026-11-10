import { useState } from "react";
import type { RequestResponse } from "@chauffeur/protocol";
import type { ElicitationItem } from "@/lib/chat";
import { ElicitationCard } from "@/components/chat/ElicitationCard";

const forms: ElicitationItem[] = [
  { kind: "elicitation", key: "boolean", ts: "", depth: 0, requestId: "boolean", serverName: "computer-use", message: "Allow access to the simulated Chrome app?", schema: { type: "object", properties: { approved: { type: "boolean", title: "Approve access" } } }, url: null, error: null, rule: null, state: "open", outcome: null, rules: null },
  { kind: "elicitation", key: "url", ts: "", depth: 0, requestId: "url", serverName: "auth", message: "Review the confirmation page, then continue.", schema: null, url: "https://example.com/confirm", error: null, rule: null, state: "open", outcome: null, rules: null },
  { kind: "elicitation", key: "reconnect", ts: "", depth: 0, requestId: "reconnect", serverName: "codex_apps", message: "Reconnect PostHog on ChatGPT to restore access for this request.", schema: null, url: "https://chatgpt.com/apps/posthog/example", error: null, rule: null, state: "resolved", outcome: "allowed", rules: null },
  { kind: "elicitation", key: "json", ts: "", depth: 0, requestId: "json", serverName: "notes", message: "Select the requested scopes.", schema: { type: "object", properties: { scopes: { type: "array", items: { type: "string" } } } }, url: null, error: null, rule: null, state: "open", outcome: null, rules: null },
  { kind: "elicitation", key: "unsupported", ts: "", depth: 0, requestId: "unsupported", serverName: "unknown", message: "This confirmation uses an unsupported schema.", schema: null, url: null, error: "Cannot accept this confirmation: unsupported form. Decline or cancel it.", rule: null, state: "open", outcome: null, rules: null },
  { kind: "elicitation", key: "blank", ts: "", depth: 0, requestId: "blank", serverName: "cua_repl", message: 'Allow Computer Use to use "Google Chrome"?', schema: { type: "object", properties: {} }, url: null, error: null, rule: 'cua_repl: Allow Computer Use to use "Google Chrome"?', state: "open", outcome: null, rules: null },
];

/** Development-only controls; responses remain in this component and never reach an agent. */
export function ElicitationFixture() {
  const [items, setItems] = useState(forms);
  const [answers, setAnswers] = useState<RequestResponse[]>([]);
  const respond = async (id: string, response: RequestResponse) => {
    setAnswers((current) => [...current, response]);
    setItems((current) => current.map((item) => item.requestId === id ? { ...item, state: "resolved", outcome: response.type === "elicitation" ? response.action : "invalid", rules: response.type === "elicitation" && response.always && item.rule ? [item.rule] : null } : item));
  };
  return <main className="mx-auto max-w-[700px] p-5">
    <h1 className="text-fg">MCP confirmations — simulated requests, no app access</h1>
    {items.map((item) => <ElicitationCard key={item.key} item={item} readOnly={false} respond={respond} />)}
    <pre data-testid="mcp-responses" className="whitespace-pre-wrap text-muted">{JSON.stringify(answers, null, 2)}</pre>
  </main>;
}
