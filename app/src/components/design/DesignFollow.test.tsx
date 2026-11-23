import { expect, test } from "bun:test";
import type { CanvasResult, DesignViewport } from "@chauffeur/protocol";
import { Window } from "happy-dom";
import { act, useLayoutEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
import { layoutCanvas, pageSections } from "@/lib/canvas";
import { DesignFollowControls, useDesignFollow } from "./DesignFollow";
import { useCanvasView } from "./useCanvasView";

const section = { title: "Checkout", screens: [{ file: "cart.html", width: 1000, height: 800 }] };
const canvas: CanvasResult = {
  chat: "design", dir: "design", pages: ["Draft", "Final"].map((name) => ({ name, sections: [section] })),
  unsorted: [],
};
const view: DesignViewport = { page: 1, scale: 1, center_x: 0.5, center_y: 0.5, presentation: null, fullscreen: null };

function Canvas({ remote }: { remote: DesignViewport | null }) {
  const [page, setPage] = useState(0);
  const layout = useMemo(() => layoutCanvas(pageSections(canvas, page)), [page]);
  const follow = useDesignFollow(remote ? [{ id: "session", name: "Bastien", view: remote }] : []);
  const { localView, moveView, setArea } = useCanvasView(layout, page, canvas, setPage, follow);
  useLayoutEffect(() => setArea({ width: 800, height: 600 }), [setArea]);
  return <>
    <DesignFollowControls follow={follow} />
    <button onClick={() => moveView((current) => ({ ...current, x: current.x + 30 }))}>Pan</button>
    <output>{JSON.stringify(localView)}</output>
  </>;
}

test("the reader can move independently, rejoin the latest view, and keep control when the presenter reconnects", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const container = document.createElement("div");
  const root = createRoot(container);
  const render = (remote: DesignViewport | null) => act(() => root.render(<Canvas remote={remote} />));
  const click = async (label: string) => {
    const button = Array.from(container.querySelectorAll("button")).find((el) => el.textContent?.trim() === label);
    if (!button) throw new Error(`Missing button: ${label}`);
    await act(() => button.click());
  };
  const viewport = () => JSON.parse(container.querySelector("output")!.textContent!);
  try {
    await render(view);
    await click("Follow Bastien");
    expect(viewport()).toEqual(view);
    await click("Pan");
    const independent = viewport();
    expect(independent.center_x).toBeLessThan(view.center_x);
    const latest = { ...view, page: 0, scale: 2, center_x: 0.25 };
    await render(latest);
    expect(viewport()).toEqual(independent);
    await click("Resume following");
    expect(viewport()).toEqual(latest);
    await render(null);
    await render(view);
    expect(container.textContent).toContain("Follow Bastien");
    expect(viewport()).toEqual(latest);
  } finally {
    await act(() => root.unmount());
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
