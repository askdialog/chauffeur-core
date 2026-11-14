import { describe, expect, test } from "bun:test";
import type { WorkspaceInfo } from "@chauffeur/protocol";
import { childRequests, childrenNeedingInput } from "./childRequests";

function workspace(id: string, extra: Partial<WorkspaceInfo> = {}): WorkspaceInfo {
  return {
    id,
    slug: id,
    title: null,
    parent_id: null,
    state: "ready",
    activity: "idle",
    created_at: `2026-11-14T09:00:0${id.length}Z`,
    ...extra,
  } as WorkspaceInfo;
}

describe("childRequests", () => {
  test("turns a child's open requests into the cards its chat shows", () => {
    const child = workspace("child", { parent_id: "parent" });
    const requests = childRequests({
      workspace: child,
      open_requests: [
        {
          chat_id: "c1",
          request: {
            type: "question",
            request_id: "q1",
            questions: [{ question: "Drop the event?", header: "Event", options: [{ label: "Drop", description: "" }], multi_select: false }],
          },
        },
        { chat_id: "c2", request: { type: "permission_request", request_id: "p1", tool_name: "Bash", input: { command: "ls" }, title: null } },
      ],
    } as never);
    expect(requests.map((r) => [r.chatId, r.item.kind, r.item.requestId, r.item.state])).toEqual([
      ["c1", "question", "q1", "open"],
      ["c2", "permission", "p1", "open"],
    ]);
    expect(requests[0].workspace).toBe(child);
  });

  test("an empty list gives no card", () => {
    expect(childRequests({ workspace: workspace("child"), open_requests: [] })).toEqual([]);
  });
});

describe("childrenNeedingInput", () => {
  test("keeps the parent's ready children that wait on the user, oldest first", () => {
    const workspaces = Object.fromEntries(
      [
        workspace("bbb", { parent_id: "p", activity: "needs_input" }),
        workspace("a", { parent_id: "p", activity: "needs_input" }),
        workspace("cc", { parent_id: "p", activity: "working" }),
        workspace("dddd", { parent_id: "other", activity: "needs_input" }),
        workspace("eeeee", { parent_id: "p", activity: "needs_input", state: "archived" }),
      ].map((w) => [w.id, w]),
    );
    expect(childrenNeedingInput(workspaces, "p").map((w) => w.id)).toEqual(["a", "bbb"]);
  });
});
