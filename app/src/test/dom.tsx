import { test } from "bun:test";
import { Window } from "happy-dom";
import { act, type ReactNode } from "react";

type DomTest = {
  render: (children: ReactNode) => Promise<void>;
  click: (selector: string) => Promise<void>;
  fill: (selector: string, value: string) => Promise<void>;
  until: (check: () => boolean) => Promise<void>;
};

export function domTest(name: string, file: string, run: (dom: DomTest) => Promise<void>) {
  test(name, async () => {
    const identity = `${file}#${name}`;
    if (process.env.CHAUFFEUR_DOM_TEST !== identity) {
      const pattern = `^${name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`;
      const child = Bun.spawn([process.execPath, "test", file, "--test-name-pattern", pattern], {
        env: { ...process.env, CHAUFFEUR_DOM_TEST: identity },
        stdout: "pipe", stderr: "pipe",
      });
      const [status, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
      if (status !== 0) throw new Error(`${stdout}\n${stderr}`);
      return;
    }
    await withDom(run);
  }, 15_000);
}

async function withDom(run: (dom: DomTest) => Promise<void>) {
  const browser = new Window();
  const globals = {
    window: browser, document: browser.document, localStorage: browser.localStorage,
    navigator: browser.navigator, HTMLElement: browser.HTMLElement, Node: browser.Node, Event: browser.Event,
    requestAnimationFrame: browser.requestAnimationFrame.bind(browser),
    cancelAnimationFrame: browser.cancelAnimationFrame.bind(browser), IS_REACT_ACT_ENVIRONMENT: true,
  };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { createRoot } = await import("react-dom/client");
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  try {
    await run({
      render: async (children) => { await act(async () => { root.render(children); }); },
      click: async (selector) => {
        const button = document.querySelector<HTMLButtonElement>(selector);
        if (!button) throw new Error(`Missing button: ${selector}`);
        await act(async () => { button.click(); });
      },
      fill: async (selector, value) => {
        const input = document.querySelector<HTMLInputElement>(selector);
        if (!input) throw new Error(`Missing input: ${selector}`);
        await act(async () => {
          const prototype = input.tagName === "TEXTAREA" ? browser.HTMLTextAreaElement.prototype : browser.HTMLInputElement.prototype;
          const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
          setter?.call(input, value);
          input.dispatchEvent(new Event("input", { bubbles: true }));
        });
      },
      until: async (check) => {
        const deadline = Date.now() + 3000;
        while (!check()) {
          if (Date.now() > deadline) throw new Error(`UI did not reach the expected state: ${document.body.textContent}`);
          await act(async () => { await new Promise((resolve) => setTimeout(resolve, 5)); });
        }
      },
    });
  } finally {
    await act(async () => { root.unmount(); });
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
}

export function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
