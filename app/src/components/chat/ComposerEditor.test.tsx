import { expect, test } from "bun:test";
import { Window } from "happy-dom";
import { act, type ComponentProps } from "react";
import { createRoot } from "react-dom/client";
import { ComposerEditor } from "./ComposerEditor";

type Key = { key: string; keyCode?: number; shiftKey?: boolean; metaKey?: boolean; ctrlKey?: boolean; isComposing?: boolean };

async function withEditor(run: (editor: {
  box: HTMLElement;
  press: (key: Key) => Promise<boolean>;
  keys: string[];
  changes: string[];
  render: (props?: Partial<ComponentProps<typeof ComposerEditor>>) => Promise<void>;
}) => Promise<void>, userAgent?: string) {
  const browser = new Window();
  if (userAgent) Object.defineProperty(browser.navigator, "userAgent", { value: userAgent });
  const globals = {
    window: browser,
    document: browser.document,
    MutationObserver: browser.MutationObserver,
    requestAnimationFrame: browser.requestAnimationFrame.bind(browser),
    cancelAnimationFrame: browser.cancelAnimationFrame.bind(browser),
    IS_REACT_ACT_ENVIRONMENT: true,
  };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const keys: string[] = [];
  const changes: string[] = [];
  const render = async (props: Partial<ComponentProps<typeof ComposerEditor>> = {}) => {
    await act(() =>
      root.render(
        <ComposerEditor
          value="bonjour"
          placeholder="Message"
          onChange={(text) => changes.push(text)}
          onPaste={() => {}}
          onKeyDown={(event) => {
            keys.push(event.key);
            if (event.key === "Enter" && !event.shiftKey) event.preventDefault();
          }}
          {...props}
        />,
      ),
    );
  };
  try {
    await render();
    const box = container.querySelector<HTMLElement>("[data-testid=composer]");
    if (!box) throw new Error("no composer");
    await run({
      box, keys, changes, render,
      press: async (key) => {
        const event = new browser.KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...key });
        // happy-dom's DOM types differ from the browser's, though these are DOM events.
        await act(() => { box.dispatchEvent(event as unknown as Event); });
        return event.defaultPrevented;
      },
    });
  } finally {
    await act(() => root.unmount());
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

test("Enter reaches the composer before the editor can swallow the key", async () => {
  await withEditor(async ({ box, press, keys, changes }) => {
    // CodeMirror filters WebKit's first key after composition before running its DOM handlers.
    box.addEventListener("keydown", (event) => event.stopImmediatePropagation(), { capture: true });
    expect(await press({ key: "Enter", keyCode: 13 })).toBe(true);
    expect(keys).toEqual(["Enter"]);
    expect(changes).toEqual([]);
  });
});

test("Enter confirming an unfinished composition never sends the message", async () => {
  await withEditor(async ({ press, keys }) => {
    await press({ key: "Enter", keyCode: 13, isComposing: true });
    // Safari can emit compositionend first, leaving isComposing false on the confirming key.
    await press({ key: "Enter", keyCode: 229 });
    expect(keys).toEqual([]);
  });
});

test("Shift Enter inserts a newline and modified Enter reaches the composer", async () => {
  await withEditor(async ({ press, keys, changes }) => {
    await press({ key: "Enter", keyCode: 13, shiftKey: true });
    expect(changes).toEqual(["bonjour\n"]);
    expect(await press({ key: "Enter", keyCode: 13, metaKey: true })).toBe(true);
    expect(await press({ key: "Enter", keyCode: 13, ctrlKey: true })).toBe(true);
    expect(keys).toEqual(["Enter", "Enter", "Enter"]);
  });
});

test("the mounted editor uses the latest composer handler for Tab and Escape", async () => {
  await withEditor(async ({ render, press, keys }) => {
    const latest: string[] = [];
    await render({ onKeyDown: (event) => { latest.push(event.key); event.preventDefault(); } });
    expect(await press({ key: "Tab", keyCode: 9 })).toBe(true);
    expect(await press({ key: "Escape", keyCode: 27 })).toBe(true);
    expect(keys).toEqual([]);
    expect(latest).toEqual(["Tab", "Escape"]);
  });
});

test("mobile shortcuts wait for the editor to deliver deferred keys", async () => {
  for (const userAgent of ["iPhone Mobile Safari", "Android Chrome"]) {
    await withEditor(async ({ box, press, keys }) => {
      const defer = (event: Event) => event.stopImmediatePropagation();
      box.addEventListener("keydown", defer, { capture: true });
      await press({ key: "Enter", keyCode: 13 });
      expect(keys).toEqual([]);
      box.removeEventListener("keydown", defer, { capture: true });
      expect(await press({ key: "Enter", keyCode: 13 })).toBe(true);
      expect(keys).toEqual(["Enter"]);
    }, userAgent);
  }
});
