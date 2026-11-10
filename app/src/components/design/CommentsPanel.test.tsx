import { expect, test } from "bun:test";
import { Window } from "happy-dom";
import { act, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import type { Thread } from "@/lib/comments";
import { CommentComposer, ThreadView } from "./CommentsPanel";

type Key = { key: string; metaKey?: boolean; ctrlKey?: boolean; shiftKey?: boolean };

/** Renders a composer holding "Too dark", presses `keys` in its text box, and says what was called. */
async function press(keys: Key[], props: Partial<ComponentProps<typeof CommentComposer>> = {}) {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const calls: string[] = [];
  try {
    await act(() =>
      root.render(
        <CommentComposer
          initial="Too dark"
          placeholder="Your comment"
          submitLabel="Comment"
          onSubmit={(text) => calls.push(`submit ${text}`)}
          onSend={(text) => calls.push(`send ${text}`)}
          onCancel={() => calls.push("cancel")}
          {...props}
        />,
      ),
    );
    const box = container.querySelector("textarea");
    if (!box) throw new Error("no text box");
    for (const key of keys) {
      await act(() => {
        box.dispatchEvent(new browser.KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...key }) as unknown as Event);
      });
    }
    return { calls, hint: container.textContent ?? "" };
  } finally {
    await act(() => root.unmount());
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

test("Enter writes the comment, ⌘Enter or Ctrl+Enter writes and sends it", async () => {
  expect((await press([{ key: "Enter" }])).calls).toEqual(["submit Too dark"]);
  expect((await press([{ key: "Enter", metaKey: true }])).calls).toEqual(["send Too dark"]);
  expect((await press([{ key: "Enter", ctrlKey: true }])).calls).toEqual(["send Too dark"]);
  expect((await press([{ key: "Enter", shiftKey: true }])).calls).toEqual([]);
});

test("the hint names ⌘⏎ only where the comment can be sent", async () => {
  expect((await press([])).hint).toContain("⌘⏎ send");
  const reviewer = await press([{ key: "Enter", metaKey: true }], { onSend: undefined });
  expect(reviewer.hint).not.toContain("⌘⏎");
  expect(reviewer.calls).toEqual(["submit Too dark"]);
});

test("nothing is sent while busy or empty", async () => {
  expect((await press([{ key: "Enter", metaKey: true }], { busy: true })).calls).toEqual([]);
  expect((await press([{ key: "Enter", metaKey: true }], { initial: "  " })).calls).toEqual([]);
});

test("@ lists the people of the share; picking one names them in what is submitted", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, requestAnimationFrame: (fn: () => void) => setTimeout(fn, 0), IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const submitted: unknown[] = [];
  const people = [
    { name: "Alice", email: "alice@example.com" },
    { name: "Bob Martin", email: "bob@example.com" },
  ];
  try {
    await act(() =>
      root.render(
        <CommentComposer
          placeholder="Your comment"
          submitLabel="Comment"
          mentionSetup={{ people, me: "alice@example.com" }}
          onSubmit={(text, mentions) => submitted.push({ text, mentions })}
          onCancel={() => submitted.push("cancel")}
        />,
      ),
    );
    const box = container.querySelector("textarea");
    if (!box) throw new Error("no text box");
    const type = async (value: string) => {
      await act(() => {
        const setter = Object.getOwnPropertyDescriptor(browser.HTMLTextAreaElement.prototype, "value")?.set;
        setter?.call(box, value);
        box.setSelectionRange(value.length, value.length);
        // React-dom was loaded before this window: it follows a text box's value by its focus
        // and its keys, as for a browser without input events.
        box.dispatchEvent(new browser.FocusEvent("focusin", { bubbles: true }) as unknown as Event);
        box.dispatchEvent(new browser.KeyboardEvent("keyup", { bubbles: true, key: "x" }) as unknown as Event);
      });
    };
    const key = async (name: string) => {
      await act(() => {
        box.dispatchEvent(new browser.KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: name }) as unknown as Event);
      });
    };
    await type("Ask @");
    // The writer is never offered.
    expect([...container.querySelectorAll("[data-testid=mention-option]")].map((o) => o.textContent)).toEqual(["Bob Martinbob@example.com"]);
    // Esc closes the list, not the comment.
    await key("Escape");
    expect(container.querySelector("[data-testid=mention-list]")).toBeNull();
    expect(submitted).toEqual([]);
    await type("Ask @bo");
    await key("Enter");
    expect(box.value).toBe("Ask @Bob Martin ");
    await type("Ask @Bob Martin about it");
    await key("Enter");
    expect(submitted).toEqual([{ text: "Ask @Bob Martin about it", mentions: [{ name: "Bob Martin", email: "bob@example.com" }] }]);
  } finally {
    await act(() => root.unmount());
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});

test("a thread is sent alone to the agent from its popover, never from a reviewer's", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const author = { name: "Elio", email: "elio@example.com" };
  const thread: Thread = { comment: { id: "c1", screen: "a.html", author, at: 1, text: "Une seule ligne ?" }, replies: [], number: 2 };
  const sent: string[] = [];
  const view = (onSendThread?: (id: string) => void) => (
    <ThreadView thread={thread} me={null} busy={false} onReply={() => {}} onSendThread={onSendThread} onResolve={() => {}} onEdit={() => {}} onDelete={() => {}} />
  );
  try {
    await act(() => root.render(view((id) => sent.push(id))));
    const button = container.querySelector<HTMLButtonElement>("[data-testid=comment-send-thread]");
    if (!button) throw new Error("no send button");
    await act(() => {
      button.click();
    });
    expect(sent).toEqual(["c1"]);
    await act(() => root.render(view()));
    expect(container.querySelector("[data-testid=comment-send-thread]")).toBeNull();
  } finally {
    await act(() => root.unmount());
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
