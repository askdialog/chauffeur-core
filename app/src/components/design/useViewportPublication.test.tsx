import { expect, jest, test } from "bun:test";
import type { DesignViewport } from "@chauffeur/protocol";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { followPeers, useViewportPublication } from "./useViewportPublication";

const view: DesignViewport = { page: 0, scale: 1, center_x: 0.5, center_y: 0.5, fullscreen: null, presentation: null };

test("another tab with the same identity can be followed", () => {
  const author = { name: "Lee", email: "lee@example.com" };
  const sessions = ["owner:local", "reviewer:1"].map(session => ({ session, author, view }));
  expect(followPeers(sessions, "owner:local")).toEqual([{ id: "reviewer:1", name: "Lee", view }]);
});

test("an active owner publishes the latest motion, renews while idle, and withdraws when disabled", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const root = createRoot(document.createElement("div"));
  const sent: (DesignViewport | null)[] = [];
  const publish = (value: DesignViewport | null) => { sent.push(value); };
  function Owner({ current, enabled }: { current: DesignViewport; enabled: boolean }) {
    useViewportPublication(current, enabled, publish, true);
    return null;
  }
  const render = (current: DesignViewport, enabled = true) => act(() => root.render(<Owner current={current} enabled={enabled} />));
  jest.useFakeTimers();
  try {
    await render(view, false);
    expect(sent).toEqual([]);
    await render(view);
    await render({ ...view, center_x: 1 });
    const moved = { ...view, center_x: 2 };
    await render(moved);
    expect(sent).toEqual([view]);
    jest.advanceTimersByTime(68);
    expect(sent).toEqual([view, moved]);
    jest.advanceTimersByTime(10_000);
    expect(sent).toEqual([view, moved, moved]);
    await render(moved, false);
    jest.advanceTimersByTime(20_000);
    expect(sent).toEqual([view, moved, moved, null]);
  } finally {
    jest.useRealTimers();
    await act(() => root.unmount());
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
