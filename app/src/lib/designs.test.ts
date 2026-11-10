import { describe, expect, test } from "bun:test";
import type { ChatInfo, SharedDesign, WorkspaceInfo } from "@chauffeur/protocol";
import { designOptions, designsOf, designWorkspaces } from "./designs";

const ws = (id: string, state: string) => ({ id, state }) as WorkspaceInfo;
const chat = (id: string, workspace_id: string, kind: string, created_at: string, closed = false) =>
  ({ id, workspace_id, kind, created_at, closed }) as unknown as ChatInfo;

describe("the Office's Design window", () => {
  test("lists the open design chats, newest first, with their workspace", () => {
    const one = ws("one", "ready");
    const two = ws("two", "creating");
    const designs = designsOf(
      [
        chat("old", "one", "design", "2026-10-20T10:00:00Z"),
        chat("code", "one", "code", "2026-10-26T10:00:00Z"),
        chat("new", "two", "design", "2026-10-25T10:00:00Z"),
        chat("closed", "one", "design", "2026-10-26T11:00:00Z", true),
      ],
      [one, two],
    );
    expect(designs.map((d) => [d.chat.id, d.workspace.id])).toEqual([
      ["new", "two"],
      ["old", "one"],
    ]);
  });

  test("leaves out the design chats of a workspace archived, being archived or unknown", () => {
    const designs = designsOf(
      [chat("a", "gone", "design", "2026-10-26T10:00:00Z"), chat("b", "going", "design", "2026-10-26T10:00:00Z"), chat("c", "nowhere", "design", "2026-10-26T10:00:00Z")],
      [ws("gone", "archived"), ws("going", "archiving")],
    );
    expect(designs).toEqual([]);
  });

  test("offers a new design chat only in a workspace ready or being created", () => {
    const offered = designWorkspaces([ws("a", "ready"), ws("b", "creating"), ws("c", "archived"), ws("d", "cleanup_failed")]);
    expect(offered.map((w) => w.id)).toEqual(["a", "b"]);
  });
});

describe("the designs a new workspace can start with", () => {
  const shared = (id: string, named: boolean, team: boolean) =>
    ({ id, url: `https://design.example/s/${id}`, title: `Share ${id}`, owner: { name: "Bob", email: "bob@example.com" }, named, team, online: true, created_at: 0 }) as SharedDesign;

  test("the user's own by their chat, colleagues' by their link, in the Design window's sections", () => {
    const one = { ...ws("one", "ready"), slug: "checkout" } as WorkspaceInfo;
    const options = designOptions(
      [{ chat: { ...chat("c1", "one", "design", "2026-10-26T10:00:00Z"), title: "Cart" } as ChatInfo, workspace: one }],
      [shared("aa", true, false), shared("bb", false, true), shared("cc", true, true)],
    );
    expect(options.yours.map((o) => [o.title, o.detail, o.design])).toEqual([["Cart", "checkout", { kind: "chat", chat: "c1" }]]);
    expect(options.named.map((o) => o.design)).toEqual([
      { kind: "share", url: "https://design.example/s/aa" },
      { kind: "share", url: "https://design.example/s/cc" },
    ]);
    expect(options.team.map((o) => [o.title, o.detail])).toEqual([["Share bb", "Bob"]]);
  });
});
