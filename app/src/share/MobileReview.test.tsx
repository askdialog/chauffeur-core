import { expect, test } from "bun:test";
import { Window } from "happy-dom";
import { act, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { MobileReview } from "./MobileReview";
type Props = ComponentProps<typeof MobileReview>;
const draftKey = "chauffeur:review-draft:abcdef01:me@example.com:home.html:screen";
async function fixture(run: (ui: { container: HTMLElement; render: (props: Partial<Props>) => Promise<void>; sent: unknown[]; click: (text: string) => Promise<void> }) => Promise<void>, target = "home.html") {
  const browser = new Window({ url: `https://design.chauffeur.askdialog.com/s/abcdef01?mobile=1&native=1&screen=${target}` });
  const globals = { window: browser, document: browser.document, location: browser.location, history: browser.history, sessionStorage: browser.sessionStorage, ResizeObserver: browser.ResizeObserver, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  browser.sessionStorage.setItem(draftKey, JSON.stringify({ anchor: { file: "home.html", x: 35, y: 45, whole: false, element: null }, text: "Keep the parent visible", reply: "", thread: "old-thread" }));
  const container = document.createElement("div"); document.body.append(container);
  const root = createRoot(container); const sent: unknown[] = [];
  let props: Props = { id: "abcdef01", title: "Mobile UX", owner: "Aline", me: { email: "me@example.com", name: "Me" }, canvas: { chat: "review", dir: "", pages: [], unsorted: ["home.html", "other.html"].map(file => ({ file, label: file, width: 390, height: 844 })) }, comments: [], online: false, connected: true, snapshotEpoch: 1, onReconnect: () => {}, say: message => { sent.push(message); return true; }, serviceError: null, pins: {}, stale: new Set(), onFrame: () => {}, elementUnder: async () => null };
  const render = async (next: Partial<Props>) => { props = { ...props, ...next }; await act(() => root.render(<MobileReview {...props} />)); };
  const click = async (text: string) => { const button = [...container.querySelectorAll("button")].find(item => item.textContent?.trim() === text || item.getAttribute("aria-label") === text); if (!button) throw Error(`No button ${text}`); await act(() => button.click()); };
  try { await render({}); await run({ container, render, sent, click }); }
  finally { await act(() => root.unmount()); await browser.happyDOM.close(); for (const [key, descriptor] of previous) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key); } }
}
test("posting waits for a new service comment by this author", async () => {
  await fixture(async ({ container, render, sent, click }) => {
    expect(container.textContent).toContain("The owner’s Mac is offline");
    await click("Post comment");
    expect(sent).toEqual([{ type: "add", screen: "home.html", pin: { x: 35, y: 45, element: undefined }, text: "Keep the parent visible" }]);
    expect(container.textContent).toContain("Posting comment…");
    const comment = { id: "comment1", screen: "home.html", text: "Keep the parent visible", author: { email: "other@example.com", name: "Other" }, at: Date.now() };
    await render({ comments: [comment] }); expect(container.textContent).not.toContain("Comment posted");
    await render({ comments: [comment, { ...comment, id: "comment2", author: { email: "me@example.com", name: "Me" } }] });
    expect(container.textContent).toContain("Comment posted"); expect(sessionStorage.getItem(draftKey)).toBeNull(); expect(location.search).toContain("thread=comment2");
  });
});
test("service refusal retains the draft and leaving requires explicit discard", async () => {
  await fixture(async ({ container, render, click }) => {
    await click("Post comment"); await render({ serviceError: "Comment not posted" });
    expect(container.querySelector("textarea")?.value).toBe("Keep the parent visible");
    await click("Cancel comment"); expect(container.querySelector('[role="dialog"]')).not.toBeNull();
    await click("Keep editing"); expect(container.querySelector("textarea")?.value).toBe("Keep the parent visible");
    await click("Cancel comment"); await click("Discard comment"); expect(container.querySelector("textarea")).toBeNull(); expect(sessionStorage.getItem(draftKey)).toBeNull();
  });
});
test("a reconnect snapshot reconciles an unconfirmed send before retry", async () => {
  await fixture(async ({ container, render, click }) => {
    await click("Post comment"); await render({ connected: false }); expect(container.textContent).toContain("Posting comment…");
    await render({ connected: true, snapshotEpoch: 2, comments: [] }); expect(container.textContent).toContain("This comment was not found after reconnecting"); expect(container.querySelector("textarea")?.value).toBe("Keep the parent visible");
    expect([...container.querySelectorAll("button")].find(button => button.textContent === "Retry")?.disabled).toBe(false);
  });
});
test("an explicit different screen wins over a cached draft and thread", async () => {
  await fixture(async ({ container, click }) => {
    expect(container.querySelector("h1")?.textContent).toBe("other.html"); expect(container.textContent).not.toContain("no longer available"); expect(container.querySelector("textarea")).toBeNull(); expect(sessionStorage.getItem(draftKey)).not.toBeNull();
    await click("Screens"); await click("home.html"); expect(container.querySelector("textarea")?.value).toBe("Keep the parent visible"); expect(location.search).toContain("screen=home.html"); expect(location.search).not.toContain("thread=");
  }, "other.html");
});
test("a thread mentioning the reader is offered, opened, then read", async () => {
  await fixture(async ({ container, render, click }) => {
    const seen: string[] = [];
    const me = { email: "me@example.com", name: "Me" };
    const comment = { id: "t1", screen: "home.html", text: "@Me can you check?", author: { email: "other@example.com", name: "Other" }, at: Date.now(), mentions: [me] };
    await render({ comments: [comment], unreadThreads: ["t1"], onSeen: (thread) => seen.push(thread) });
    await click("Cancel comment"); await click("Discard comment");
    await click("1 mention for you · Open");
    expect(seen).toEqual(["t1"]);
    expect(container.querySelector("[data-testid=mention]")?.textContent).toBe("@Me");
  });
});
