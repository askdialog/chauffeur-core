import { expect, spyOn, test } from "bun:test";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import type { DeckResult, FileCommentsResult } from "@chauffeur/protocol";

test("a deck opens beside its chat with its numbered slides, their notes and its versions", async () => {
  const browser = new Window();
  const globals = {
    window: browser, document: browser.document, localStorage: browser.localStorage, IS_REACT_ACT_ENVIRONMENT: true,
    ResizeObserver: class {
      constructor(private readonly onResize: (entries: Array<{ contentRect: { width: number; height: number } }>) => void) {}
      observe() { this.onResize([{ contentRect: { width: 128, height: 500 } }]); }
      disconnect() {}
    },
  };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { Deliverables } = await import("../Deliverables");
  const { DeliverableCard } = await import("../DeliverableCard");
  const { client, useApp } = await import("@/state/app");
  const before = useApp.getState().connection;
  useApp.setState({ connection: { status: "open", daemonVersion: "test" } });
  const path = ".chauffeur/artifacts/q3/deck.json";
  const deck: DeckResult = {
    path, title: "Q3 review", width: 1920, height: 1080, brand_changed: false,
    slides: [
      { file: "01.html", title: "Title", notes: "Welcome", skip: false, url: "http://slides.test/01.html?deck=1", version: "1" },
      { file: "02.html", title: "Plan", notes: "", skip: true, url: "http://slides.test/02.html?deck=1", version: "1" },
    ],
  };
  const calls: Array<[string, unknown]> = [];
  const call = spyOn(client, "call").mockImplementation(async (method, params) => {
    calls.push([method, params]);
    if (method === "deck.read" || method === "deck.change" || method === "deck.edit_text") return deck;
    if (method === "deliverables.versions") return [{ n: 2, at: "2026-11-15T10:00:00Z", by: "agent" }];
    if (method === "files.comments") return { path: "", comments: [], author: { name: "Me", email: "me@x" } } satisfies FileCommentsResult;
    throw new Error(`unexpected ${method}`);
  });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await act(async () => {
      root.render(
        <Deliverables workspaceId="w" revision={0}>
          <DeliverableCard document={{ title: "Q3 review", path, exports: [], kind: "deck" }} streaming={false} />
        </Deliverables>,
      );
    });
    const open = [...container.querySelectorAll("button")].find((b) => b.textContent?.trim() === "Open deck");
    await act(async () => { open?.click(); });
    expect(container.querySelector('[data-testid="deck-reader"]')).not.toBeNull();
    const thumbnails = container.querySelectorAll<HTMLElement>('[data-testid="deck-thumbnail"]');
    expect(thumbnails.length).toBe(2);
    expect(thumbnails[1]?.textContent).toContain("Plan");
    const notes = container.querySelector<HTMLTextAreaElement>('[data-testid="deck-notes"]');
    expect(notes?.value).toBe("Welcome");
    expect(calls.some(([m]) => m === "deliverables.versions")).toBe(true);

    const preview = () => container.querySelector('[data-testid="deck-stage"] iframe');
    const firstFrame = preview();
    expect(firstFrame?.getAttribute("loading")).toBe("eager");
    expect(thumbnails[0]?.querySelector("iframe")?.getAttribute("loading")).toBe("lazy");
    await act(async () => { container.querySelector<HTMLButtonElement>('[aria-label="Reload"]')?.click(); });
    expect(preview()).not.toBe(firstFrame);
    expect(preview()?.getAttribute("src")).toBe(firstFrame?.getAttribute("src"));

    const actions = thumbnails[0]?.querySelector<HTMLButtonElement>('[aria-label="Actions for slide 1"]');
    await act(async () => { actions?.click(); });
    expect(notes?.value).toBe("Welcome");
    expect(calls.some(([m]) => m === "deck.change")).toBe(false);
    await act(async () => { actions?.click(); });
    await act(async () => { thumbnails[1]?.click(); });
    expect(preview()?.getAttribute("src")).toBe(deck.slides[1]?.url);
    expect(calls.some(([m]) => m === "deck.change")).toBe(false);

    await act(async () => { actions?.click(); });
    const duplicate = [...document.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')].find((item) => item.textContent === "Duplicate");
    expect(duplicate).toBeDefined();
    await act(async () => { duplicate?.click(); });
    expect(calls.find(([method]) => method === "deck.change")?.[1]).toMatchObject({ change: { op: "duplicate", file: "01.html" } });
    expect(preview()?.getAttribute("src")).toBe(deck.slides[1]?.url);
  } finally {
    await act(() => root.unmount());
    call.mockRestore();
    useApp.setState({ connection: before });
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
