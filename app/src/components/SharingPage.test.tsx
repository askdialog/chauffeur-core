import { expect, spyOn, test } from "bun:test";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import type { OutgoingShare, OutgoingShareSource } from "@chauffeur/protocol";

test("archived shares remain visible and each kind stops through its own RPC", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, localStorage: browser.localStorage, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { SharingPage } = await import("./SharingPage");
  const { client } = await import("@/state/app");
  const sources: OutgoingShareSource[] = [{ kind: "design", chat: "chat" }, { kind: "workspace" }, { kind: "file", path: ".chauffeur/report.pdf" }];
  const rows: OutgoingShare[] = sources.map((source, index) => ({ id: String(index), name: source.kind, workspace: "ws", workspace_name: "Project", archived: true, url: "https://example.test", source, connection: { state: "online" } }));
  const calls: string[] = [];
  let pendingRefresh: ((rows: OutgoingShare[]) => void) | undefined;
  let delay = false;
  const rpc = spyOn(client, "call").mockImplementation(async (method) => { calls.push(method); if (method === "shares.list" && delay) return new Promise<OutgoingShare[]>(resolve => { pendingRefresh = resolve; }); return method === "shares.list" ? rows : {}; });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => root.render(<SharingPage />));
    expect(container.querySelectorAll("article").length).toBe(3);
    expect(container.textContent).toContain("Archived source");
    delay = true;
    const refresh = [...container.querySelectorAll("button")].find(button => button.textContent?.includes("Refresh"));
    await act(async () => refresh?.click());
    for (let index = 0; index < 3; index++) {
      const stop = [...container.querySelectorAll("button")].find(button => button.textContent === "Stop sharing");
      expect(stop).toBeDefined();
      await act(async () => stop?.click());
    }
    expect(calls).toEqual(["shares.list", "shares.list", "design.share_stop", "workspace.share_stop", "files.share_stop"]);
    await act(async () => pendingRefresh?.(rows));
    expect(container.textContent).toContain("Nothing shared yet");
  } finally {
    await act(async () => root.unmount());
    rpc.mockRestore(); container.remove();
    for (const [key, value] of previous) { if (value) Object.defineProperty(globalThis, key, value); else Reflect.deleteProperty(globalThis, key); }
  }
});
