// Development only (`http://localhost:1420/#chat-fixture`): the chat view fed with fabricated
// events, to work on its rendering without the agent sidecar. Not part of the production bundle.
// `?stream#chat-fixture` streams an answer at the end, a token every 100 ms.
// `?long&stream#chat-fixture` exercises measurement and scrolling across 400 exchanges.

import type { ChatEvent, ChatInfo, EventEnvelope } from "@chauffeur/protocol";
import { useEffect, useMemo, useState } from "react";
import { ChatConversation } from "@/components/chat/ChatPanel";
import { buildChatView, reduceChat } from "@/lib/chat";

let seq = 0;
const p = (event: ChatEvent): EventEnvelope => ({
  seq: ++seq,
  ts: new Date(Date.UTC(2026, 9, 13, 9, 0, seq)).toISOString(),
  event: { scope: "chat", workspace_id: "w", chat_id: "c", event },
});
const e = (event: ChatEvent): EventEnvelope => ({
  seq: null,
  ts: new Date().toISOString(),
  event: { scope: "chat", workspace_id: "w", chat_id: "c", event },
});

// Earlier exchanges, enough for the chat to scroll a long way (and for its timeline rail).
const topics = [
  "Why does the cart page flash on load? It renders twice before the prices show up.",
  "Rename `useCartTotals` to `useCartSummary` everywhere, including the tests.",
  "The price filter ignores discounts: a 20% off item still shows under its full price bracket. Can you find where the bracket is computed and fix it?",
  "Add a loading skeleton to the product grid.",
  "Run the whole test suite and tell me what fails.",
  "Explain how the session cookie is refreshed.",
  "Make the header sticky on mobile only.",
  "Write a migration that backfills `orders.currency` from the store settings.",
];
const exchanges = new URLSearchParams(location.search).has("long")
  ? Array.from({ length: 400 }, (_, i) => topics[i % topics.length])
  : topics;
const history: ChatEvent[] = exchanges.flatMap((text, i): ChatEvent[] => [
  { type: "user_message", text, queued: false },
  { type: "turn_started" },
  { type: "tool_call", tool_use_id: `h${i}a`, name: "Grep", input: { pattern: "cart", path: "alpha/src" }, parent_tool_use_id: null },
  { type: "tool_result", tool_use_id: `h${i}a`, output: "alpha/src/cart.tsx:12", is_error: false },
  { type: "tool_call", tool_use_id: `h${i}b`, name: "Read", input: { file_path: "/ws/alpha/src/cart.tsx" }, parent_tool_use_id: null },
  { type: "tool_result", tool_use_id: `h${i}b`, output: "export function CartPage() {}", is_error: false },
  {
    type: "assistant_text",
    message_id: `h${i}m`,
    text: Array.from({ length: 2 + (i % 4) * 2 }, (_, k) => `Paragraph ${k + 1} about "${text.slice(0, 30)}…": what I found, what I changed and why, with enough words to wrap over a couple of lines in a normal window.`).join("\n\n"),
    parent_tool_use_id: null,
  },
  { type: "turn_ended", outcome: "success", duration_ms: 12000 + i * 1000, cost_usd: 0.05 * (i + 1), context_tokens: 12000, error: null },
]);

/** Persisted, `seconds` ago: running tasks show how long they have run. */
const ago = (seconds: number, event: ChatEvent): EventEnvelope => ({
  ...p(event),
  ts: new Date(Date.now() - seconds * 1000).toISOString(),
});

const events: EventEnvelope[] = [
  p({ type: "session_started", agent_session_id: "s1", model: "opus" }),
  ...history.map(p),
  p({ type: "user_message", text: "Add a checkout button to the cart page and expose `POST /checkout` in the API.", queued: false }),
  p({ type: "turn_started" }),
  p({ type: "thinking", message_id: "m1", text: "The cart lives in alpha/src/cart.tsx; the API routes are in beta/api/routes.py.\nI should read both first." }),
  p({ type: "tool_call", tool_use_id: "t1", name: "Bash", input: { command: "rg -n \"CartPage\" alpha/src", description: "Find the cart page" }, parent_tool_use_id: null }),
  p({ type: "tool_result", tool_use_id: "t1", output: "alpha/src/cart.tsx:12:export function CartPage() {\nalpha/src/routes.tsx:8:  { path: \"/cart\", element: <CartPage /> },", is_error: false }),
  p({ type: "tool_call", tool_use_id: "t2", name: "Read", input: { file_path: "/ws/checkout-flow/alpha/src/cart.tsx" }, parent_tool_use_id: null }),
  p({ type: "tool_result", tool_use_id: "t2", output: "export function CartPage() {\n  return <Cart />;\n}", is_error: false }),
  p({ type: "tool_call", tool_use_id: "t3", name: "Task", input: { description: "Explore the API routes", prompt: "Find where routes are declared in beta" }, parent_tool_use_id: null }),
  p({ type: "tool_call", tool_use_id: "t3a", name: "Grep", input: { pattern: "@router.post", path: "beta/api" }, parent_tool_use_id: "t3" }),
  p({ type: "tool_result", tool_use_id: "t3a", output: "beta/api/routes.py:40:@router.post(\"/orders\")", is_error: false }),
  p({ type: "assistant_text", message_id: "sub1", text: "Routes are declared in `beta/api/routes.py` with `@router.post`.", parent_tool_use_id: "t3" }),
  p({ type: "tool_result", tool_use_id: "t3", output: "Routes are declared in beta/api/routes.py", is_error: false }),
  p({
    type: "tool_call",
    tool_use_id: "t4",
    name: "Edit",
    input: { file_path: "/ws/checkout-flow/alpha/src/cart.tsx", old_string: "  return <Cart />;", new_string: "  return (\n    <Cart>\n      <CheckoutButton />\n    </Cart>\n  );" },
    parent_tool_use_id: null,
  }),
  p({ type: "tool_result", tool_use_id: "t4", output: "The file was updated.", is_error: false }),
  p({ type: "tool_call", tool_use_id: "t5", name: "Bash", input: { command: "bun test alpha" }, parent_tool_use_id: null }),
  p({ type: "tool_result", tool_use_id: "t5", output: "error: 1 test failed\n  cart > renders the checkout button", is_error: true }),
  p({
    type: "assistant_text",
    message_id: "m2",
    text: "I added the button. What changed:\n\n- **Button:** `CheckoutButton` sits under the cart total and stays disabled while the cart is empty or a price is still loading, so nobody pays for a stale amount.\n- **Route:** the API gets `POST /checkout`, which turns the cart into an order.\n\nNext steps:\n\n1. Fix the failing test\n2. Add the route:\n\n```python\n@router.post(\"/checkout\")\nasync def checkout(cart: Cart) -> Order:\n    return await orders.create(cart)\n```\n\n| repo | change |\n|---|---|\n| alpha | `CheckoutButton` |\n| beta | `POST /checkout` |\n\n```mermaid\nsequenceDiagram\n  participant C as Cart\n  participant A as API\n  C->>A: POST /checkout\n  A-->>C: 201 Created\n```",
    parent_tool_use_id: null,
  }),
  p({ type: "turn_ended", outcome: "success", duration_ms: 48200, cost_usd: 0.42, context_tokens: 38100, error: null }),
  p({ type: "user_message", text: "Go ahead, and run the API tests too.", queued: false }),
  p({ type: "turn_started" }),
  p({ type: "permission_request", request_id: "r1", tool_name: "Bash", input: { command: "docker compose up -d postgres" }, title: "Start the test database?" }),
  // The agent proposes a repo the workspace lacks (story 48).
  p({
    type: "permission_request",
    request_id: "r3",
    tool_name: "mcp__chauffeur__add_repo",
    input: {
      repo: "analytics",
      reason: "The checkout events are read by the analytics dashboards: their schema has to follow the new order status.",
      description: "Dashboards and metrics of the Dialog assistant",
    },
    title: "Add analytics to this workspace?",
  }),
  p({ type: "status_changed", status: "needs_input" }),
  p({
    type: "question",
    request_id: "q1",
    questions: [
      {
        question: "Which status code should POST /checkout return?",
        header: "API",
        multi_select: false,
        options: [
          { label: "201 Created", description: "With the order in the body" },
          { label: "202 Accepted", description: "Payment is processed asynchronously" },
        ],
      },
      {
        question: "Which clients need the new endpoint?",
        header: "Clients",
        multi_select: true,
        options: [
          { label: "Web", description: "" },
          { label: "Mobile", description: "" },
        ],
      },
    ],
  }),
  ago(95, { type: "task_started", task_id: "b1", kind: "command", description: "Run the API test suite in watch mode", subagent_type: null, tool_use_id: "t9", background: true, parent_task_id: null }),
  ago(40, { type: "task_started", task_id: "a1", kind: "subagent", description: "Review the checkout flow", subagent_type: "code-reviewer", tool_use_id: "t10", background: true, parent_task_id: null }),
  e({ type: "task_progress", task_id: "a1", activity: null, last_tool: "Read", summary: "Reading beta/api/orders.py", duration_ms: 40_000, tool_uses: 7 }),
  // The subagent's own tasks, nested under it in the tasks bar: a command, and a subagent with its own command.
  ago(30, { type: "task_started", task_id: "a1c", kind: "command", description: "sleep 300", subagent_type: null, tool_use_id: "t11", background: true, parent_task_id: "a1" }),
  ago(25, { type: "task_started", task_id: "a2", kind: "subagent", description: "Check the tax rules", subagent_type: "Explore", tool_use_id: "t12", background: true, parent_task_id: "a1" }),
  ago(12, { type: "task_started", task_id: "a2c", kind: "command", description: "npm run watch:tax", subagent_type: null, tool_use_id: "t13", background: true, parent_task_id: "a2" }),
  p({
    type: "permission_request",
    request_id: "r2",
    tool_name: "Bash",
    input: { command: "git log --oneline -20 beta/api/orders.py" },
    title: "Claude wants to run git log --oneline -20 beta/api/orders.py",
    agent: { id: "a1", subagent_type: "code-reviewer", description: "Review the checkout flow", background: true },
  }),
  p({ type: "user_message", text: "Also update the changelog.", queued: true }),
  p({ type: "notice", level: "warning", message: "Claude usage limit nearly reached (seven_day), resets at 2026-10-21T11:00:00.000Z" }),
  p({ type: "notice", level: "error", message: "Claude usage limit reached (five_hour), resets at 2026-10-21T11:00:00.000Z" }),
  p({ type: "notice", level: "warning", message: "Chauffeur restarted and interrupted the agent; send a message to resume" }),
  e({ type: "thinking_delta", message_id: "m3", text: "Waiting for the answer before running the tests" }),
  e({ type: "text_delta", message_id: "m4", text: "While you decide, I am reading `beta/api/orders.py`" }),
];

const chat: ChatInfo = {
  id: "c",
  kind: "chat",
  mcp_servers: [
    { name: "claude.ai Linear", status: "needs_auth", error: null, failed_call: true, missing_env: [], uses_env: [], declared_in: null },
    { name: "gitnexus", status: "connected", error: null, failed_call: false, missing_env: [], uses_env: [], declared_in: null },
    { name: "playwright", status: "failed", error: "spawn npx ENOENT", failed_call: false, missing_env: [], uses_env: [], declared_in: null },
    {
      name: "slack",
      status: "failed",
      error: "Connection closed",
      failed_call: false,
      missing_env: ["SLACK_MCP_TOKEN"],
      uses_env: ["SLACK_MCP_TOKEN"],
      declared_in: "~/.claude.json",
    },
  ],
  workspace_id: "w",
  agent: "claude",
  model: "opus",
  effort: null,
  fast: false,
  permission_mode: "default",
  title: "Checkout",
  status: "needs_input",
  agent_session_id: "s1",
  created_at: "2026-10-13T09:00:00Z",
  running_tasks: 5,
  closed: false,
  last_activity_at: null,
};

export function ChatFixture() {
  const initial = useMemo(() => buildChatView(events), []);
  const [view, setView] = useState(initial);
  useEffect(() => {
    if (!new URLSearchParams(location.search).has("stream")) return;
    let n = 0;
    const timer = setInterval(() => {
      n += 1;
      setView((current) => reduceChat(current, e({ type: "text_delta", message_id: "m4", text: n % 12 === 0 ? "\n\n" : ` token${n}` })));
    }, 100);
    return () => clearInterval(timer);
  }, []);
  return (
    <div className="flex h-full flex-col">
      <div className="flex h-9 shrink-0 items-center border-b border-line px-4 text-[12px] text-faint">
        Chat fixture: fabricated events (development only)
      </div>
      <ChatConversation chat={chat} view={view} readOnly={false} archived={false} />
    </div>
  );
}
