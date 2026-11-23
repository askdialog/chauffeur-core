import { expect, spyOn, test } from "bun:test";
import type { DesignViewport, ShareSnapshot, ToReviewer } from "@chauffeur/protocol";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { api } from "./api";
import { SharePage } from "./SharePage";

test("a reviewer keeps the presented variant when opening its pin or stopping, and leaves it on Close", async () => {
  const me = { name: "Reader", email: "reader@example.com" };
  const owner = { name: "Presenter", email: "presenter@example.com" };
  const screens = ["a.html", "b.html"].map(file => ({ file, width: 300, height: 300 }));
  const snapshot: ShareSnapshot = {
    id: "abcdef01", title: "Design", me, owner, reviewers: [], people: [me, owner], files: [],
    comments: [{ id: "thread", screen: "b.html", author: me, at: 1, text: "Second variant" }],
    unread_mentions: [], viewers: [], online: true,
    canvas: { chat: "chat", dir: "", pages: [{ name: "Page", sections: [{ title: "Variants", screens }] }], unsorted: [] },
  };
  const browser = new Window({ url: "https://example.test/s/abcdef01" });
  let connection: Socket;
  class Socket {
    static OPEN = 1;
    readyState = 1;
    sent: string[] = [];
    onopen = () => {};
    onmessage = (_event: { data: string }) => {};
    constructor() { connection = this; }
    send(frame: string) { this.sent.push(frame); }
    close() {}
  }
  class SizeObserver {
    constructor(readonly resize: () => void) {}
    observe(el: HTMLElement) {
      Object.defineProperties(el, { clientWidth: { value: 900, configurable: true }, clientHeight: { value: 800, configurable: true } });
      this.resize();
    }
    disconnect() {}
  }
  const globals = { document: browser.document, window: browser, location: browser.location, history: browser.history,
    PointerEvent: browser.PointerEvent, matchMedia: browser.matchMedia.bind(browser), ResizeObserver: SizeObserver, WebSocket: Socket, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { value, configurable: true });
  const who = spyOn(api, "me").mockResolvedValue(me);
  const read = spyOn(api, "snapshot").mockResolvedValue(snapshot);
  const root = createRoot(document.body.appendChild(document.createElement("div")));
  const dialog = () => document.querySelector<HTMLElement>('[aria-label="Present"]');
  const click = async (label: string) => {
    const btn = Array.from((dialog() ?? document).querySelectorAll("button")).find(el => el.textContent?.trim() === label);
    if (!btn) throw new Error(`Missing ${label}`);
    await act(() => btn.click());
  };
  try {
    await act(async () => root.render(<SharePage />));
    expect(connection!.sent).toEqual([]);
    await act(() => connection.onopen());
    expect(connection!.sent).toHaveLength(1);
    const position: DesignViewport = { page: 0, scale: 1, center_x: 0.5, center_y: 0.5, presentation: null, fullscreen: null };
    const update = (view: DesignViewport) => {
      const message: ToReviewer = { type: "participants", session: "self", participants: [{ session: "leader", author: owner, view }] };
      return act(() => connection.onmessage({ data: JSON.stringify(message) }));
    };
    const highlighted = () => document.querySelector('[data-testid="canvas-selected"]')?.previousElementSibling?.textContent;
    await update({ ...position, selected: 0 });
    await click("Follow Presenter");
    expect(highlighted()).toBe("a.html");
    await update({ ...position, selected: 1 });
    expect(highlighted()).toBe("b.html");
    await click("Stop");
    expect(highlighted()).toBe("b.html");

    await click("Follow Presenter");
    const screen = document.querySelector<HTMLElement>('[data-file="a.html"]')!;
    await act(() => {
      for (const type of ["pointerdown", "pointerup"]) {
        screen.dispatchEvent(new PointerEvent(type, { bubbles: true, pointerId: 1, button: 0 }));
      }
    });
    expect(highlighted()).toBe("a.html");
    await update({ ...position, selected: 1, presentation: 1 });
    expect(highlighted()).toBe("a.html");
    expect(dialog()).toBeNull();
    await click("Resume following");
    expect(dialog()?.textContent).toContain("2 / 2");
    await act(() => dialog()!.querySelector<HTMLButtonElement>('[data-testid="canvas-pin"]')!.click());
    expect(dialog()?.textContent).toContain("Second variant");
    expect(dialog()?.textContent).toContain("You have control");
    await click("Resume following");
    await click("Stop");
    expect(dialog()?.textContent).toContain("2 / 2");
    expect(highlighted()).toBe("b.html");
    await click("Follow Presenter");
    await act(() => dialog()!.querySelector<HTMLButtonElement>('[data-testid="canvas-close"]')!.click());
    expect(dialog()).toBeNull();
  } finally {
    await act(() => root.unmount());
    who.mockRestore();
    read.mockRestore();
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
