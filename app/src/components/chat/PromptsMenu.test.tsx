import { expect, spyOn } from "bun:test";
import type { Prompt } from "@chauffeur/protocol";
import { EditorView } from "@codemirror/view";
import { act } from "react";
import { deferred, domTest } from "@/test/dom";

const file = import.meta.path;

type Key = { key: string; shiftKey?: boolean; ctrlKey?: boolean; metaKey?: boolean };

/** A chat's composer whose daemon holds `prompts` (read again on each `prompts.list`). */
async function setup(render: (children: React.ReactNode) => Promise<void>, prompts: () => Prompt[] | Promise<Prompt[]>, draft = "") {
  // MutationObserver for CodeMirror, which domTest does not install.
  Object.defineProperty(globalThis, "MutationObserver", { configurable: true, value: window.MutationObserver });
  const { client, useApp } = await import("@/state/app");
  const { drafts } = await import("@/lib/drafts");
  const { ComposerBox } = await import("./ChatPanel");
  const calls: string[] = [];
  const sent: string[] = [];
  spyOn(client, "call").mockImplementation(async (method: string) => {
    calls.push(method);
    if (method === "prompts.list") return prompts();
    if (method === "commands.list") return { commands: [{ name: "compact", description: "Compact the conversation", argument_hint: null }] };
    throw new Error(`Unexpected RPC: ${method}`);
  });
  drafts.setText("c1", draft);
  await render(
    <ComposerBox
      draftKey="c1"
      working={false}
      placeholder="Message the agent…"
      commandsFor={{ workspaceId: "w1", agent: "claude" }}
      connectorSetup={false}
      suggestion={{ text: "Run the tests", key: "s1" }}
      onSend={async (text) => { sent.push(text); }}
      onInterrupt={() => {}}
      controls={null}
    />,
  );
  const box = document.querySelector<HTMLElement>("[data-testid=composer]");
  if (!box) throw new Error("no composer");
  const view = EditorView.findFromDOM(box);
  if (!view) throw new Error("no editor");
  const press = async (target: Element, key: Key) => {
    const event = new window.KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...key });
    await act(async () => { target.dispatchEvent(event as unknown as Event); });
  };
  return {
    calls,
    sent,
    useApp,
    /** What the composer holds, its selection drawn as [from|to] (a lone caret as |). */
    composer: () => {
      const { from, to } = view.state.selection.main;
      const doc = view.state.doc.toString();
      return from === to ? `${doc.slice(0, from)}|${doc.slice(from)}` : `${doc.slice(0, from)}[${doc.slice(from, to)}]${doc.slice(to)}`;
    },
    composerFocused: () => document.activeElement === view.contentDOM,
    type: async (text: string) => {
      await act(async () => { view.dispatch(view.state.replaceSelection(text), { userEvent: "input.type" }); });
    },
    select: async (anchor: number, head = anchor) => {
      await act(async () => { view.dispatch({ selection: { anchor, head } }); });
    },
    pressInComposer: (key: Key) => press(view.contentDOM, key),
    pressInMenu: (key: Key) => press(filter() ?? document.body, key),
  };
}

const menu = () => document.querySelector("[data-testid=prompts-menu]");
const filter = () => document.querySelector<HTMLInputElement>("[data-testid=prompts-filter]");
const rows = () => [...document.querySelectorAll('[data-testid=prompts-menu] [role=option]')].map((row) => row.querySelector("span")?.textContent);
const selected = () => document.querySelector('[data-testid=prompts-menu] [aria-selected=true] span')?.textContent;
const suggestionShown = () => document.querySelector("[data-testid=prompt-suggestion]") !== null;
const shiftTab = { key: "Tab", shiftKey: true };

const saved: Prompt[] = [
  { name: "agreed", text: "Agreed, carry on" },
  { name: "Compact", text: "/compact Keep the decisions" },
];

domTest("⇧Tab opens the prompts menu whether the composer is empty, shows the suggestion, holds text or the / menu, which closes", file, async ({ render, until }) => {
  const c = await setup(render, () => saved);

  // Empty, with Claude's suggestion: it stays offered after Esc.
  expect(suggestionShown()).toBe(true);
  await c.pressInComposer(shiftTab);
  await until(() => rows().length === 2);
  expect(document.activeElement).toBe(filter());
  await c.pressInMenu({ key: "Escape" });
  expect(menu()).toBeNull();
  expect(c.composer()).toBe("|");
  expect(c.composerFocused()).toBe(true);
  expect(suggestionShown()).toBe(true);

  // With text: it is left as it was, the caret where it was.
  await c.type("Hello world");
  await c.select(6);
  await c.pressInComposer(shiftTab);
  await until(() => rows().length === 2);
  await c.pressInMenu({ key: "Escape" });
  expect(c.composer()).toBe("Hello |world");
  expect(c.composerFocused()).toBe(true);

  // The / menu closes for it, and stays closed once the prompts are dismissed.
  await c.select(0, 11);
  await c.type("/com");
  await until(() => document.querySelector("[data-testid=slash-menu] [role=option]") !== null);
  await c.pressInComposer(shiftTab);
  await until(() => rows().length === 2);
  expect(document.querySelector("[data-testid=slash-menu]")).toBeNull();
  await c.pressInMenu({ key: "Escape" });
  expect(c.composer()).toBe("/com|");
  expect(document.querySelector("[data-testid=slash-menu]")).toBeNull();
  expect(c.sent).toEqual([]);
});

domTest("the menu reads the prompts at each opening, filters them by name and text, and inserts the chosen one at the caret", file, async ({ render, until, fill }) => {
  let prompts: () => Prompt[] | Promise<Prompt[]> = () => saved;
  const c = await setup(render, () => prompts(), "Hello world");
  await c.select(6);

  await c.pressInComposer(shiftTab);
  await until(() => rows().length === 2);
  expect(rows()).toEqual(["agreed", "Compact"]);
  await c.pressInMenu({ key: "Escape" });

  // A prompt saved since shows at the next opening, the list read before shown no more meanwhile.
  const reading = deferred<Prompt[]>();
  prompts = () => reading.promise;
  await c.pressInComposer(shiftTab);
  expect(rows()).toEqual([]);
  expect(menu()?.textContent).toContain("Reading your prompts");
  await act(async () => reading.resolve([...saved, { name: "yes", text: "Yes, go ahead" }]));
  await until(() => rows().length === 3);
  expect(c.calls.filter((method) => method === "prompts.list")).toHaveLength(2);

  // ↑↓ move, wrapping around.
  expect(selected()).toBe("agreed");
  await c.pressInMenu({ key: "ArrowUp" });
  expect(selected()).toBe("yes");
  await c.pressInMenu({ key: "ArrowDown" });
  await c.pressInMenu({ key: "ArrowDown" });
  expect(selected()).toBe("Compact");

  // The filter matches names and texts ignoring case, and never lands in the composer.
  await fill("[data-testid=prompts-filter]", "GO AH");
  expect(rows()).toEqual(["yes"]);
  await fill("[data-testid=prompts-filter]", "compact");
  expect(rows()).toEqual(["Compact"]);
  await fill("[data-testid=prompts-filter]", "nothing like it");
  expect(rows()).toEqual([]);
  expect(menu()?.textContent).toContain('No prompt matches "nothing like it"');
  await c.pressInMenu({ key: "Enter" });
  expect(menu()).not.toBeNull();
  expect(c.composer()).toBe("Hello |world");

  // ↵ inserts at the caret, the caret after it; nothing is sent.
  await fill("[data-testid=prompts-filter]", "agreed");
  await c.pressInMenu({ key: "Enter" });
  expect(menu()).toBeNull();
  expect(c.composer()).toBe("Hello Agreed, carry on|world");
  expect(c.composerFocused()).toBe(true);

  // Over the selection, if any, here by a click.
  await c.select(6, 22);
  await c.pressInComposer(shiftTab);
  await until(() => rows().length === 3);
  await act(async () => {
    document.querySelectorAll("[data-testid=prompts-menu] [role=option]")[2]?.dispatchEvent(new window.MouseEvent("mousedown", { bubbles: true, cancelable: true }) as unknown as Event);
  });
  expect(menu()).toBeNull();
  expect(c.composer()).toBe("Hello Yes, go ahead|world");

  // A click outside, on the chat (its scroller takes the focus) or nothing focusable, leaves the
  // composer as it was, focused, the caret where it was; a click into another text field keeps it.
  const scroller = document.createElement("div");
  scroller.tabIndex = 0;
  const field = document.createElement("input");
  document.body.append(scroller, field);
  const clickOutside = async (target: HTMLElement) => {
    await act(async () => {
      target.dispatchEvent(new window.MouseEvent("mousedown", { bubbles: true, cancelable: true }) as unknown as Event);
      // As the browser does once the mousedown is handled.
      if (target !== document.body) target.focus();
    });
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 10)); });
  };
  await c.select(5);
  for (const target of [document.body, scroller, field]) {
    await c.pressInComposer(shiftTab);
    await until(() => rows().length === 3);
    await clickOutside(target);
    expect(menu()).toBeNull();
    expect(c.composer()).toBe("Hello| Yes, go aheadworld");
    expect(c.composerFocused()).toBe(target !== field);
  }
  expect(c.sent).toEqual([]);
  expect(c.calls.filter((method) => method !== "prompts.list")).toEqual([]);

  // The text goes as typed when the user sends it.
  await c.select(0, 24);
  await c.pressInComposer(shiftTab);
  await until(() => rows().length === 3);
  await fill("[data-testid=prompts-filter]", "compact");
  await c.pressInMenu({ key: "Enter" });
  await c.pressInComposer({ key: "Enter" });
  await until(() => c.sent.length === 1);
  expect(c.sent).toEqual(["/compact Keep the decisions"]);
});

domTest("with no prompt saved the menu says so and opens Settings › Prompts", file, async ({ render, until, click }) => {
  const c = await setup(render, () => []);
  await c.pressInComposer(shiftTab);
  await until(() => document.querySelector("[data-testid=prompts-menu-empty]") !== null);
  expect(menu()?.textContent).toContain("No prompt saved yet");
  await click("[data-testid=prompts-menu-settings]");
  expect(menu()).toBeNull();
  expect(c.useApp.getState().settingsOpen).toBe(true);
  expect(c.useApp.getState().settingsSection).toBe("prompts");
});

domTest("Tab takes the suggestion and the / menu's command; ⇧Tab takes neither", file, async ({ render, until }) => {
  const c = await setup(render, () => saved);
  await c.pressInComposer({ key: "Tab" });
  expect(c.composer()).toBe("Run the tests|");
  expect(menu()).toBeNull();

  await c.select(0, 13);
  await c.type("/com");
  await until(() => document.querySelector("[data-testid=slash-menu] [role=option]") !== null);
  await c.pressInComposer({ key: "Tab" });
  expect(c.composer()).toBe("/compact |");

  await c.select(0, 9);
  await c.type("/com");
  await until(() => document.querySelector("[data-testid=slash-menu] [role=option]") !== null);
  await c.pressInComposer(shiftTab);
  await until(() => rows().length === 2);
  expect(c.composer()).toBe("/com|");

  // In the prompts menu, as in the / menu, Tab inserts; ⇧Tab keeps the focus there.
  await c.pressInMenu(shiftTab);
  expect(document.activeElement).toBe(filter());
  await c.pressInMenu({ key: "Tab" });
  expect(menu()).toBeNull();
  expect(c.composer()).toBe("/comAgreed, carry on|");
});
