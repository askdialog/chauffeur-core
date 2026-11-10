import { expect, test } from "bun:test";
import type { ShareComment } from "@chauffeur/protocol";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import type { ShareThread } from "@/lib/shareComments";
import { type ShareComments, ShareCommentCards, ShareCommentsContext } from "./ShareComments";

const alice = { name: "Alice", email: "alice@example.com" };
const bob = { name: "Bob", email: "bob@example.com" };

const comment = (id: string, more: Partial<ShareComment> = {}): ShareComment => ({
  id,
  chat: "c1",
  message: "m1",
  quote: "a monolith",
  author: bob,
  text: "@Alice why?",
  at: 10,
  mentions: [{ email: alice.email, name: "Alice" }],
  ...more,
});

/** Renders one thread's card for Alice, with `more` of the context, and lets `act` on it. */
async function card(thread: ShareThread, more: Partial<ShareComments>, act_: (container: HTMLElement, browser: Window) => Promise<void> = async () => {}) {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const context: ShareComments = { byItem: new Map([["s1", [thread]]]), me: alice.email, onDelete: null, onReply: null, ...more };
  try {
    await act(() =>
      root.render(
        <ShareCommentsContext.Provider value={context}>
          <ShareCommentCards itemKey="s1" />
        </ShareCommentsContext.Provider>,
      ),
    );
    await act_(container, browser);
    return container.innerHTML;
  } finally {
    await act(() => root.unmount());
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

test("a thread shows its replies, the reader's mention highlighted, and @you while unread", async () => {
  const thread = { comment: comment("k1"), replies: [comment("r1", { author: alice, text: "Because", quote: "", reply_to: "k1", mentions: [] })] };
  const unread = await card(thread, { unread: new Set(["k1"]) });
  expect(unread).toContain("a monolith");
  expect(unread).toContain("Because");
  expect(unread).toContain('data-testid="mention"');
  expect(unread).toContain("You are mentioned here");
  expect(await card(thread, {})).not.toContain("You are mentioned here");
});

test("only who may comment replies, and only the author deletes", async () => {
  const thread = { comment: comment("k1"), replies: [comment("r1", { author: alice, text: "Mine", quote: "", reply_to: "k1", mentions: [] })] };
  const deleted: string[] = [];
  const reader = await card(thread, { onDelete: (c) => deleted.push(c.id) });
  expect(reader).not.toContain("share-comment-reply-open");
  const replies: string[] = [];
  await card(thread, { onDelete: (c) => deleted.push(c.id), onReply: (t, text) => (replies.push(`${t.id} ${text}`), true) }, async (container, browser) => {
    const buttons = container.querySelectorAll<HTMLButtonElement>('[data-testid="share-comment-delete"]');
    expect(buttons.length).toBe(1);
    await act(() => buttons[0]?.click());
    await act(() => container.querySelector<HTMLButtonElement>('[data-testid="share-comment-reply-open"]')?.click());
    const box = container.querySelector("textarea");
    if (!box) throw new Error("no reply box");
    await act(() => {
      const setter = Object.getOwnPropertyDescriptor(browser.HTMLTextAreaElement.prototype, "value")?.set;
      setter?.call(box, "Agreed");
      // React-dom was loaded before this window: it follows a text box by its focus and keys.
      box.dispatchEvent(new browser.FocusEvent("focusin", { bubbles: true }) as unknown as Event);
      box.dispatchEvent(new browser.KeyboardEvent("keyup", { bubbles: true, key: "x" }) as unknown as Event);
    });
    await act(() => {
      box.dispatchEvent(new browser.KeyboardEvent("keydown", { key: "Enter", bubbles: true, cancelable: true }) as unknown as Event);
    });
  });
  expect(deleted).toEqual(["r1"]);
  expect(replies).toEqual(["k1 Agreed"]);
});
