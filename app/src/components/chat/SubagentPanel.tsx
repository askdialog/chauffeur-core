// A subagent's conversation, in a panel over the right of the chat: opened from its Agent call in
// the chat or its line in the tasks bar. Live from the chat's events (its text never enters the
// chat itself), and from its transcript (`chat.subagent_messages`) for what came before: after it
// finished, an app reload, an idle close or a daemon restart. A nested subagent opens in the same
// panel, with a breadcrumb back.

import type { ChatInfo, SubagentMessagesResult } from "@chauffeur/protocol";
import { ChevronDown, ChevronRight, Square, Users, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useReducer, useState } from "react";
import type { ChatView } from "@/lib/chat";
import { groupActivity } from "@/lib/chat";
import { formatDuration, formatElapsed } from "@/lib/format";
import { fromVimEditor } from "@/lib/keys";
import { errorMessage } from "@/lib/rpc";
import {
  liveItems,
  mergeConversation,
  panelStack,
  type SubagentStatus,
  type SubagentTarget,
  subagentSummary,
  targetToolUse,
  transcriptItems,
  withoutReport,
} from "@/lib/subagent";
import { client, toast } from "@/state/app";
import { Markdown } from "../Markdown";
import { Badge, Button, cx, IconButton, Spinner } from "../ui";
import { ActivityContext, ChatRowView, type Respond, SubagentOpenContext } from "./ChatItems";

/** Which subagents are open in a chat: the one shown last, the ones it was opened from before it. */
export function useSubagentPanel() {
  const [stack, dispatch] = useReducer(panelStack, []);
  const open = useCallback((target: SubagentTarget) => dispatch({ type: "open", target }), []);
  const push = useCallback((target: SubagentTarget) => dispatch({ type: "push", target }), []);
  const back = useCallback((index: number) => dispatch({ type: "back", index }), []);
  const close = useCallback(() => dispatch({ type: "close" }), []);
  return { stack, open, push, back, close };
}

const STATUS_TONE: Record<SubagentStatus, "working" | "success" | "danger" | "neutral"> = {
  running: "working",
  completed: "success",
  failed: "danger",
  stopped: "neutral",
};

/** The transcript as read: none yet, loading, or the answer. */
type Transcript = { state: "loading" } | { state: "read"; result: SubagentMessagesResult } | { state: "failed"; error: string };

const noRespond: Respond = async () => {};

export function SubagentPanel({
  chat,
  view,
  stack,
  readOnly,
  onPush,
  onBack,
  onClose,
}: {
  chat: ChatInfo;
  view: ChatView;
  stack: SubagentTarget[];
  readOnly: boolean;
  onPush: (target: SubagentTarget) => void;
  onBack: (index: number) => void;
  onClose: () => void;
}) {
  const target = stack[stack.length - 1]!;
  const toolUseId = targetToolUse(view, target);
  const live = useMemo(() => liveItems(view, toolUseId), [view, toolUseId]);
  const [transcript, setTranscript] = useState<Transcript>({ state: "loading" });
  const now = useNow(true);
  const provisional = subagentSummary(view, target, live, now);
  const running = provisional.status === "running";

  // Read on opening, and again once it ends: the transcript then holds all of it.
  useEffect(() => {
    let cancelled = false;
    setTranscript((current) => (current.state === "read" ? current : { state: "loading" }));
    client
      .call("chat.subagent_messages", {
        chat: chat.id,
        tool_use_id: toolUseId ?? undefined,
        agent_id: target.agentId ?? undefined,
      })
      .then((result) => !cancelled && setTranscript({ state: "read", result }))
      .catch((error) => !cancelled && setTranscript({ state: "failed", error: errorMessage(error) }));
    return () => {
      cancelled = true;
    };
  }, [chat.id, toolUseId, target.agentId, running]);

  const read = useMemo(
    () => (transcript.state === "read" && transcript.result.found ? transcriptItems(transcript.result.events) : null),
    [transcript],
  );
  const conversation = useMemo(() => mergeConversation(read, live), [read, live]);
  const summary = subagentSummary(view, target, conversation, now);
  const shown = withoutReport(conversation, summary.report);
  const rows = useMemo(() => groupActivity(shown), [shown]);

  const [openGroups, setOpenGroups] = useState<ReadonlySet<string>>(() => new Set());
  const activity = useMemo(
    () => ({
      open: openGroups,
      toggle: (key: string) =>
        setOpenGroups((current) => {
          const next = new Set(current);
          if (!next.delete(key)) next.add(key);
          return next;
        }),
    }),
    [openGroups],
  );

  // Esc closes the panel before anything else reads it (it would interrupt the agent).
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || event.defaultPrevented || fromVimEditor(event)) return;
      if (document.querySelector("[role=dialog]")) return;
      event.preventDefault();
      onClose();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  const [stopping, setStopping] = useState(false);
  const stop = async () => {
    if (!summary.taskId) return;
    setStopping(true);
    try {
      await client.call("chat.stop_task", { chat: chat.id, task_id: summary.taskId });
    } catch (error) {
      toast(`Could not stop the subagent: ${errorMessage(error)}`, "error");
    } finally {
      setStopping(false);
    }
  };

  const title = [summary.type, summary.description].filter(Boolean).join(" · ") || "Subagent";
  const loading = transcript.state === "loading";
  const unavailable = !loading && shown.length === 0 && !summary.report && !running;

  return (
    <aside
      className="absolute inset-y-0 right-0 z-20 flex w-[40%] min-w-[340px] max-w-[760px] flex-col border-l border-line-strong bg-bg shadow-2xl"
      aria-label="Subagent"
      data-testid="subagent-panel"
    >
      <div className="shrink-0 border-b border-line px-3 py-2">
        {stack.length > 1 && (
          <nav className="mb-1 flex min-w-0 items-center gap-1 text-[11px] text-faint" data-testid="subagent-breadcrumb">
            {stack.slice(0, -1).map((crumb, index) => (
              <span key={index} className="flex min-w-0 items-center gap-1">
                <button className="truncate hover:text-fg hover:underline" onClick={() => onBack(index)}>
                  {crumbLabel(view, crumb)}
                </button>
                <ChevronRight size={10} className="shrink-0" />
              </span>
            ))}
            <span className="truncate text-muted">{summary.description ?? "Subagent"}</span>
          </nav>
        )}
        <div className="flex min-w-0 items-center gap-2">
          <Users size={14} className="shrink-0 text-muted" />
          <span className="min-w-0 flex-1 truncate font-medium text-fg" title={title}>
            {title}
          </span>
          <IconButton title="Close (Esc)" onClick={onClose}>
            <X size={13} />
          </IconButton>
        </div>
        <div className="mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-[11.5px] text-faint">
          <span data-testid="subagent-status" data-status={summary.status}>
            <Badge tone={STATUS_TONE[summary.status]}>{summary.status}</Badge>
          </span>
          {summary.background && <span>background</span>}
          {summary.durationMs != null && (
            <span className="tabular-nums">{running ? formatElapsed(summary.durationMs) : formatDuration(summary.durationMs)}</span>
          )}
          {summary.toolUses != null && <span>{`${summary.toolUses} tool ${summary.toolUses === 1 ? "use" : "uses"}`}</span>}
          {running && summary.taskId && !readOnly && (
            <Button size="xs" variant="ghost" busy={stopping} onClick={() => void stop()} className="ml-auto" data-testid="subagent-stop">
              <Square size={8} fill="currentColor" /> Stop
            </Button>
          )}
        </div>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto pb-4">
        {summary.prompt && <Prompt text={summary.prompt} />}
        <SubagentOpenContext.Provider value={onPush}>
          <ActivityContext.Provider value={activity}>
            <div className="py-1" data-testid="subagent-conversation">
              {rows.map((row, index) => (
                <ChatRowView
                  key={row.key}
                  row={row}
                  last={index === rows.length - 1}
                  working={running}
                  readOnly
                  respond={noRespond}
                />
              ))}
            </div>
          </ActivityContext.Provider>
        </SubagentOpenContext.Provider>
        {loading && shown.length === 0 && (
          <div className="flex items-center gap-2 px-5 py-2 text-[12px] text-faint">
            <Spinner size={11} /> Reading its conversation…
          </div>
        )}
        {running && !loading && (
          <div className="flex items-center gap-2 px-5 py-1 text-[12px] text-faint">
            <Spinner size={11} className="text-working" /> Working…
          </div>
        )}
        {summary.report && (
          <div className="mx-5 mt-2 rounded-lg border border-line bg-panel px-3 py-2" data-testid="subagent-report">
            <div className="mb-1 text-[10.5px] uppercase tracking-wide text-faint">Report</div>
            <div className={cx("text-fg", summary.failed && "text-danger")}>
              <Markdown text={summary.report} />
            </div>
          </div>
        )}
        {unavailable && (
          <div className="px-5 py-3 text-[12.5px] text-faint" data-testid="subagent-unavailable">
            This subagent's conversation is not available.
            {transcript.state === "failed" && <div className="mt-1 text-[11.5px]">{transcript.error}</div>}
          </div>
        )}
      </div>
    </aside>
  );
}

/** What the main agent asked of the subagent, folded past a few lines. */
function Prompt({ text }: { text: string }) {
  const [open, setOpen] = useState(false);
  const long = text.length > 280 || text.split("\n").length > 4;
  return (
    <div className="mx-5 mt-3 rounded-lg border border-line bg-raised px-3 py-2" data-testid="subagent-prompt">
      <button
        className="mb-1 flex items-center gap-1 text-[10.5px] uppercase tracking-wide text-faint hover:text-muted disabled:pointer-events-none"
        onClick={() => setOpen((v) => !v)}
        disabled={!long}
      >
        {long && (open ? <ChevronDown size={10} /> : <ChevronRight size={10} />)}
        Prompt
      </button>
      <div className={cx("whitespace-pre-wrap break-words text-[12.5px] text-muted selectable", long && !open && "line-clamp-4")}>{text}</div>
    </div>
  );
}

function crumbLabel(view: ChatView, target: SubagentTarget): string {
  const summary = subagentSummary(view, target, [], Date.now());
  return summary.description ?? summary.type ?? "Subagent";
}

/** The time now, every second while `live`. */
function useNow(live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [live]);
  return now;
}
