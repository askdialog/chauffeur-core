// A child workspace's questions, permission requests and MCP confirmations, shown in its parent's
// chats too (docs/adr/child-questions-reach-the-parent.md). They are answered from here or from the
// child: the first answer wins, and the card then leaves both.

import type { RequestResponse } from "@chauffeur/protocol";
import { ArrowUpRight, MessageCircleQuestion } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { useShallow } from "zustand/react/shallow";
import { type ChildRequest, childRequests, childrenNeedingInput } from "@/lib/childRequests";
import { workspaceName } from "@/lib/format";
import { errorMessage } from "@/lib/rpc";
import { client, selectChat, selectWorkspace, toast, useApp } from "@/state/app";
import { RequestCard, WorkspacePathContext } from "./ChatItems";

/** The events after which a child's open requests may have changed. */
const REQUEST_EVENTS = new Set(["question", "permission_request", "mcp_elicitation", "request_resolved", "status_changed"]);

/** The open requests of `workspaceId`'s children, kept up to date while they wait on the user. */
export function useChildRequests(workspaceId: string): ChildRequest[] {
  const children = useApp(useShallow((s) => childrenNeedingInput(s.workspaces, workspaceId)));
  const ids = children.map((c) => c.id).join(",");
  const [byChild, setByChild] = useState<Record<string, ChildRequest[]>>({});

  useEffect(() => {
    const list = ids ? ids.split(",") : [];
    let live = true;
    const load = (id: string) =>
      client
        .call("workspace.wait", { workspace: id, timeout_secs: 0 })
        .then((result) => live && setByChild((current) => ({ ...current, [id]: childRequests(result) })))
        .catch(() => {});
    setByChild((current) => Object.fromEntries(Object.entries(current).filter(([id]) => list.includes(id))));
    list.forEach((id) => void load(id));
    if (list.length === 0) return;
    const subscription = client.subscribe(
      { all: false, registry: false, workspaces: list, chats: [], ephemeral: false },
      (envelope) => {
        const event = envelope.event;
        if (event.scope === "chat" && REQUEST_EVENTS.has(event.event.type)) void load(event.workspace_id);
      },
    );
    return () => {
      live = false;
      subscription.close();
    };
  }, [ids]);

  return useMemo(() => children.flatMap((child) => byChild[child.id] ?? []), [children, byChild]);
}

/** Answers a child's request on its own chat. */
export function respondTo(request: ChildRequest) {
  return async (requestId: string, response: RequestResponse) => {
    try {
      await client.call("chat.respond", { chat: request.chatId, request_id: requestId, response });
    } catch (error) {
      toast(`Could not answer: ${errorMessage(error)}`, "error");
    }
  };
}

function openChild(request: ChildRequest) {
  selectWorkspace(request.workspace.id);
  selectChat(request.workspace.id, request.chatId);
}

/**
 * Above the parent's composer: one line per child request, the child named with a link to its
 * chat. A permission request or MCP confirmation shows its card here; a question is answered in
 * place of the composer, as in the child, `onAnswer` bringing it back once set aside.
 */
export function ChildRequestsPanel({
  requests,
  shownQuestion,
  onAnswer,
}: {
  requests: ChildRequest[];
  /** The child question shown in place of the composer, if any. */
  shownQuestion: string | null;
  onAnswer: (requestId: string) => void;
}) {
  if (requests.length === 0) return null;
  return (
    <div className="mx-auto max-h-[45vh] w-full max-w-[860px] overflow-y-auto px-5 pb-1" data-testid="child-requests">
      {requests.map((request) => (
        <ChildRequestRow
          key={`${request.chatId}:${request.item.requestId}`}
          request={request}
          shown={shownQuestion === request.item.requestId}
          onAnswer={onAnswer}
        />
      ))}
    </div>
  );
}

function ChildRequestRow({ request, shown, onAnswer }: { request: ChildRequest; shown: boolean; onAnswer: (requestId: string) => void }) {
  const item = request.item;
  const name = workspaceName(request.workspace);
  const respond = respondTo(request);
  const what = item.kind === "question" ? "asks you" : item.kind === "permission" ? "asks your permission" : "asks you to confirm";
  return (
    <div className="my-1" data-testid="child-request" data-child={request.workspace.id}>
      <div className="flex min-w-0 items-center gap-2 text-[12px]">
        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-attention" />
        <span className="min-w-0 truncate text-muted">
          <span className="font-medium text-fg">{name}</span> {what}
          {item.kind === "question" && item.questions[0] ? `: ${item.questions[0].question}` : ""}
        </span>
        {item.kind === "question" && !shown && (
          <button className="flex shrink-0 items-center gap-1 text-accent hover:underline" onClick={() => onAnswer(item.requestId)}>
            <MessageCircleQuestion size={12} /> Answer
          </button>
        )}
        <button
          className="ml-auto flex shrink-0 items-center gap-0.5 text-accent hover:underline"
          onClick={() => openChild(request)}
          title={`Open ${name}`}
        >
          Open in {name.length > 24 ? `${name.slice(0, 23)}…` : name} <ArrowUpRight size={12} />
        </button>
      </div>
      {item.kind !== "question" && (
        <WorkspacePathContext.Provider value={request.workspace.path}>
          <RequestCard item={item} readOnly={false} respond={respond} />
        </WorkspacePathContext.Provider>
      )}
    </div>
  );
}
