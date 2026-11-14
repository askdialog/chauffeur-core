// Development-only stress fixture. No daemon, workspace data or agent is used.
import type { ChatEvent, ChatInfo, EventEnvelope } from "@chauffeur/protocol";
import { useEffect, useMemo, useState } from "react";
import { useApp } from "@/state/app";
import { ChatConversation } from "@/components/chat/ChatPanel";
import { buildChatView, emptyChatView, reduceChat } from "@/lib/chat";

const params = new URLSearchParams(location.search);
const count = Math.max(1, Number(params.get("count") ?? 100));
const paragraphs = Math.max(1, Number(params.get("paragraphs") ?? 1));
const tall = params.get("tall") ?? "none";
const chat: ChatInfo = {
  id: "scroll", workspace_id: "fixture", agent: "claude", model: "opus", effort: null,
  fast: false, permission_mode: "default", title: "Scroll audit", status: "idle",
  agent_session_id: null, created_at: "2026-11-14T00:00:00Z", running_tasks: 0,
  closed: false, last_activity_at: null, mcp_servers: [], kind: "chat",
};
let seq = 0;
const envelope = (event: ChatEvent): EventEnvelope => ({
  seq: ++seq, ts: "2026-11-14T00:00:00Z",
  event: { scope: "chat", workspace_id: chat.workspace_id, chat_id: chat.id, event },
});
const paragraph = "A paragraph with enough text to wrap across several lines when the window is narrow. The reader must be able to reach every part of this message, including its last line.";
const events = Array.from({ length: count }, (_, i) => {
  const n = tall === "all" || (tall === "first" && i === 0) || (tall === "last" && i === count - 1) ? paragraphs : 1 + i % 3;
  const delayed = i === count - 1 && params.has("late")
    ? "\n\n![Delayed audit image](https://scroll-fixture.invalid/image.svg)\n\n```mermaid\nflowchart TD\n A[Start] --> B[Measure the content] --> C[Keep the last line visible]\n```\n\n| Column | Value |\n|---|---|\n| Content | Text that wraps as the viewport changes |"
    : "";
  return [
    envelope({ type: "user_message", text: `Prompt ${i}. ${params.has("user-tall") && i === 0 ? Array.from({ length: paragraphs }, () => paragraph).join("\n\n") : "Explain this part."}`, queued: false }),
    envelope({ type: "assistant_text", message_id: `answer-${i}`, text: `Answer ${i}.\n\n${Array.from({ length: n }, (_, j) => `Section ${j}. ${paragraph}`).join("\n\n")}${delayed}\n\nEND-OF-ANSWER-${i}.`, parent_tool_use_id: null }),
  ];
}).flat();

// Build each independent exchange separately so this fixture measures scrolling, rather than
// the event replay reducer's repeated array copies for very large synthetic histories.
function fixtureView(source: EventEnvelope[]) {
  return { ...emptyChatView, items: source.flatMap(event => buildChatView([event]).items), lastSeq: source.at(-1)?.seq ?? 0 };
}

export function ScrollFixture() {
  const [partial, setPartial] = useState(params.has("partial"));
  const initial = useMemo(() => fixtureView(partial ? events.slice(-30) : events), []);
  const [view, setView] = useState(initial);
  const [streaming, setStreaming] = useState(false);
  const [narrow, setNarrow] = useState(false);
  useEffect(() => {
    if (!streaming) return;
    const timer = setInterval(() => setView(current => reduceChat(current, envelope({
      type: "text_delta", message_id: `answer-${count - 1}`, text: `\n\n${paragraph}`,
    }))), 100);
    return () => clearInterval(timer);
  }, [streaming]);
  return <div className="flex h-full flex-col">
    <div className="flex shrink-0 gap-3 p-2 text-[12px]">
      <button data-testid="audit-stream" onClick={() => setStreaming(!streaming)}>Stream {streaming ? "on" : "off"}</button>
      <button data-testid="audit-history" onClick={() => { setView(fixtureView(events)); setPartial(false); }}>Load history</button>
      <button data-testid="audit-resize" onClick={() => setNarrow(!narrow)}>Resize</button>
      <button data-testid="audit-global-find" onClick={() => {
        const index = Math.floor(count / 2);
        const target = events.find(e => e.event.scope === "chat" && e.event.event.type === "assistant_text" && e.event.event.message_id === `answer-${index}`);
        if (target?.seq != null) useApp.setState({ searchTarget: { chat: chat.id, seq: target.seq, word: `END-OF-ANSWER-${index}.` } });
      }}>Global result</button>
      <button data-testid="audit-shrink" onClick={() => setView(current => reduceChat(current, envelope({ type: "assistant_text", message_id: `answer-${count - 1}`, text: `Short answer.\n\nEND-OF-ANSWER-${count - 1}.`, parent_tool_use_id: null })))}>Shrink</button>
      <button data-testid="audit-append" onClick={() => setView(current => reduceChat(current, envelope({ type: "assistant_text", message_id: `append-${seq}`, text: "Appended answer.\n\nEND-OF-APPEND", parent_tool_use_id: null })))}>Append</button>
    </div>
    <div className="flex min-h-0 flex-1 flex-col" style={{ width: narrow ? 420 : "100%" }}>
      <ChatConversation chat={chat} view={view} readOnly archived={false} partial={partial} />
    </div>
  </div>;
}
