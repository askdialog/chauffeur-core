import { expect, spyOn, test } from "bun:test";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import type { DesignShareState } from "@chauffeur/protocol";

test("a markdown file's Share dialog lists its linked files and carries them unless unchecked", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, localStorage: browser.localStorage, navigator: browser.navigator, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { ShareDialog } = await import("./ShareDialog");
  const { client } = await import("@/state/app");
  const state: DesignShareState = { chat: "file:w:.chauffeur/s/RECAP.md", configured: true, email: "alice@example.com", connection: { state: "off" }, past_reviewers: ["bob@example.com"], comments_rev: 0 };
  const started: unknown[] = [];
  const call = spyOn(client, "call").mockImplementation((async (method: string, params: unknown) => {
    if (method === "files.linked") return { files: [".chauffeur/s/MODELISATION.md", ".chauffeur/s/AUDIT.md"], truncated: false };
    if (method === "files.share_start") {
      started.push(params);
      return state;
    }
    throw new Error(`unexpected ${method}`);
  }) as typeof client.call);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const query = (id: string) => document.querySelector<HTMLElement>(`[data-testid="${id}"]`);
  try {
    await act(async () => { root.render(<ShareDialog target={{ kind: "file", workspace: "w", path: ".chauffeur/s/RECAP.md" }} state={state} onClose={() => {}} />); });
    const box = query("share-linked") as HTMLInputElement;
    expect(box.checked).toBe(true);
    expect(box.closest("label")?.textContent).toContain("Also share the 2 linked files");
    expect([...query("share-linked-files")!.querySelectorAll("li")].map((li) => li.textContent)).toEqual([".chauffeur/s/MODELISATION.md", ".chauffeur/s/AUDIT.md"]);
    await act(async () => { query("share-suggestion")!.click(); });
    await act(async () => { query("share-start")!.click(); });
    expect(started).toEqual([{ workspace: "w", path: ".chauffeur/s/RECAP.md", reviewers: ["bob@example.com"], team: false, linked: true }]);
    await act(async () => { box.click(); });
    expect(box.checked).toBe(false);
    await act(async () => { query("share-start")!.click(); });
    expect((started[1] as { linked: boolean }).linked).toBe(false);
  } finally {
    await act(() => root.unmount());
    call.mockRestore();
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});

test("a file linking to nothing has no linked files checkbox", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, localStorage: browser.localStorage, navigator: browser.navigator, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { ShareDialog } = await import("./ShareDialog");
  const { client } = await import("@/state/app");
  const state: DesignShareState = { chat: "file:w:a.md", configured: true, email: "alice@example.com", connection: { state: "off" }, past_reviewers: [], comments_rev: 0 };
  const call = spyOn(client, "call").mockImplementation((async () => ({ files: [], truncated: false })) as unknown as typeof client.call);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => { root.render(<ShareDialog target={{ kind: "file", workspace: "w", path: "repo/a.md" }} state={state} onClose={() => {}} />); });
    expect(document.querySelector('[data-testid="share-linked"]')).toBeNull();
  } finally {
    await act(() => root.unmount());
    call.mockRestore();
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
