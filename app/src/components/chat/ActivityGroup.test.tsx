import { expect, test } from "bun:test";
import { renderToStaticMarkup } from "react-dom/server";
import type { ActivityGroup, ToolItem } from "@/lib/chat";
import { ActivityContext, ChatRowView } from "./ChatItems";

const agent: ToolItem = {
  kind: "tool", key: "agent", ts: "2026-11-16T10:00:00Z", depth: 0,
  toolUseId: "agent", parentToolUseId: null, name: "Agent",
  input: { description: "Review the checkout flow" }, summary: "Review the checkout flow", result: null,
};
const read: ToolItem = {
  ...agent, key: "read", toolUseId: "read", parentToolUseId: "agent", depth: 1,
  name: "Read", input: { file_path: "/ws/api/orders.py" }, summary: "/ws/api/orders.py",
};
const group: ActivityGroup = { kind: "activity", key: "activity", items: [agent, read] };

function render(row = group, { open = false, last = true, working = true } = {}) {
  return renderToStaticMarkup(
    <ActivityContext.Provider value={{ open: new Set(open ? [row.key] : []), toggle: () => {} }}>
      <ChatRowView row={row} last={last} working={working} readOnly respond={async () => {}} />
    </ActivityContext.Provider>,
  );
}

const spinners = (html: string) => (html.match(/animate-spin/g) ?? []).length;

test("a folded live group shows only the current subagent step's spinner", () => {
  const html = render();
  expect(html).toContain('data-tool="Read"');
  expect(spinners(html)).toBe(1);
  expect(html).not.toContain("text-success/80");
});

test("an expanded group leaves the running indicators on its tool calls", () => {
  expect(spinners(render(group, { open: true }))).toBe(2);
});

test("a folded group keeps its indicator when the pending call is hidden", () => {
  expect(spinners(render(group, { last: false }))).toBe(1);
  const completedRead = { ...read, result: { output: "done", isError: false, attachments: [] } };
  expect(spinners(render({ ...group, items: [agent, completedRead] }))).toBe(1);
});

test("a stopped group has no running indicator", () => {
  expect(spinners(render(group, { working: false }))).toBe(0);
});
