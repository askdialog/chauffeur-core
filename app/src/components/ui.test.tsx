import { expect } from "bun:test";
import { act, useState } from "react";
import { domTest } from "@/test/dom";
import { createPortal } from "react-dom";
import { Field, Modal } from "./ui";

domTest("an autofocus modal contains keyboard focus and restores its trigger", import.meta.path, async ({ render, click }) => {
  function Page() {
    const [open, setOpen] = useState(false);
    return <><button id="trigger" onClick={() => setOpen(true)}>Open</button><Modal open={open} onClose={() => setOpen(false)} title={<span>Settings</span>}><input id="last" autoFocus /></Modal></>;
  }
  await render(<Page />);
  document.getElementById("trigger")!.focus();
  await click("#trigger");
  const modal = document.querySelector<HTMLElement>('[role="dialog"]')!;
  expect(modal.getAttribute("aria-modal")).toBe("true");
  expect(document.getElementById(modal.getAttribute("aria-labelledby")!)?.textContent).toBe("Settings");
  expect(modal.contains(document.activeElement)).toBe(true);
  expect(document.getElementById("trigger")!.parentElement!.inert).toBe(true);
  document.getElementById("last")!.focus();
  await act(async () => { window.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Tab", bubbles: true })); });
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Close");
  await act(async () => { window.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Tab", shiftKey: true, bubbles: true })); });
  expect(document.activeElement?.id).toBe("last");
  await act(async () => { window.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
  expect(document.querySelector('[role="dialog"]')).toBeNull();
  expect(document.activeElement?.id).toBe("trigger");
  expect(document.getElementById("trigger")!.parentElement!.inert).toBe(false);
});

domTest("fields name nested inputs without replacing explicit accessible names", import.meta.path, async ({ render }) => {
  await render(<Field label="Daemon port" hint="Used after restart"><div><input id="port" type="number" /><input id="named" aria-label="Custom name" /></div></Field>);
  const input = document.getElementById("port")!;
  expect(document.getElementById(input.getAttribute("aria-labelledby")!)?.textContent).toBe("Daemon port");
  expect(document.getElementById(input.getAttribute("aria-describedby")!)?.textContent).toBe("Used after restart");
  expect(document.getElementById("named")?.getAttribute("aria-label")).toBe("Custom name");
  expect(document.getElementById("named")?.hasAttribute("aria-labelledby")).toBe(false);
});

domTest("a nested modal restores its parent while leaving the application inert", import.meta.path, async ({ render, click }) => {
  function Page() {
    const [child, setChild] = useState(false);
    return <><button id="background">Background</button><Modal open onClose={() => {}} title="Parent"><button id="nested" onClick={() => setChild(true)}>Nested</button><Modal open={child} onClose={() => setChild(false)} title="Child"><input id="child" /></Modal></Modal></>;
  }
  await render(<Page />);
  document.getElementById("nested")!.focus();
  await click("#nested");
  expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(2);
  await act(async () => { window.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Escape", bubbles: true })); });
  expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(1);
  expect(document.activeElement?.id).toBe("nested");
  expect(document.getElementById("background")!.parentElement!.inert).toBe(true);
});

domTest("Tab cycles across the dialog and portalled feedback without leaving the page", import.meta.path, async ({ render }) => {
  await render(<>{createPortal(<div data-global-feedback><button id="feedback">Dismiss</button><details><summary id="help">Help</summary></details></div>, document.body)}<Modal open title="Settings" onClose={() => {}}><input id="last-control" /></Modal></>);
  document.getElementById("last-control")!.focus();
  const press = async (shiftKey = false) => { await act(async () => { window.dispatchEvent(new window.KeyboardEvent("keydown", { key: "Tab", shiftKey, bubbles: true })); }); };
  await press();
  expect(document.activeElement?.id).toBe("feedback");
  await press();
  expect(document.activeElement?.id).toBe("help");
  await press();
  expect(document.activeElement?.getAttribute("aria-label")).toBe("Close");
  await press(true);
  expect(document.activeElement?.id).toBe("help");
  await press(true);
  expect(document.activeElement?.id).toBe("feedback");
  await press(true);
  expect(document.activeElement?.id).toBe("last-control");
});
