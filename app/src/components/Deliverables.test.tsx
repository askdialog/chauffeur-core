import { expect, spyOn, test } from "bun:test";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import type { DesignShareState, FileContent } from "@chauffeur/protocol";

test("a document opens beside its chat, switches format, refreshes without resetting unchanged content and closes", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, localStorage: browser.localStorage, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const { Deliverables } = await import("./Deliverables");
  const { DeliverableCard } = await import("./DeliverableCard");
  const { client, useApp } = await import("@/state/app");
  const before = useApp.getState().connection;
  useApp.setState({ connection: { status: "open", daemonVersion: "test" } });
  const html = ".chauffeur/artifacts/report/index.html";
  const pdf = ".chauffeur/artifacts/report/report.pdf";
  let version = "v1";
  const read = spyOn(client, "call").mockImplementation(async (method, params) => {
    if (method === "files.download") return "single-use-ticket";
    if (method === "files.share_state") return { chat: "file:w:test", configured: false, connection: { state: "off" }, past_reviewers: [], comments_rev: 0 } satisfies DesignShareState;
    const path = params && "path" in params ? params.path : "";
    return { path, kind: path === pdf ? "media" : "text", mime: path === pdf ? "application/pdf" : "text/html", version, size: 30, read_only: true, preview_url: "http://preview.test/index.html", text: "<h1>Report</h1>", base64: null } satisfies FileContent;
  });
  const url = spyOn(client, "httpUrl").mockImplementation((route) => `http://daemon.test${route}?token=test`);
  const opened = spyOn(browser, "open").mockReturnValue(null);
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  const render = (revision: number, exports = [pdf], primary = html) => act(async () => { root.render(
    <Deliverables workspaceId="w" revision={revision}>
      <div data-testid="conversation">Conversation and draft</div>
      <DeliverableCard document={{ title: "Report", path: primary, exports }} streaming={false} />
    </Deliverables>,
  ); });
  const click = async (label: string) => {
    const button = [...container.querySelectorAll("button")].find((b) => b.textContent?.trim() === label || b.getAttribute("aria-label") === label);
    if (!button) throw new Error(`Missing button: ${label}`);
    await act(async () => { button.click(); });
  };
  try {
    await render(0);
    expect(container.querySelector("iframe")).toBeNull();
    await click("Open preview");
    expect(container.querySelector('[data-testid="conversation"]')).not.toBeNull();
    const frame = container.querySelector("iframe");
    expect(frame?.getAttribute("sandbox")).toBe("allow-scripts");
    expect(container.textContent).toContain("Download HTML");
    await click("Download HTML");
    expect(opened.mock.calls[0]?.[0]).toBe("http://daemon.test/deliverables/w?ticket=single-use-ticket");
    await render(1);
    expect(container.querySelector("iframe")).toBe(frame);
    version = "v2";
    await render(2);
    expect(container.querySelector("iframe")).not.toBe(frame);
    await click("PDF");
    expect(container.querySelector("iframe")?.src).toContain("/media/w");
    expect(container.textContent).toContain("Download PDF");
    await render(3, []);
    await click("Open preview");
    expect(container.querySelector("iframe")?.getAttribute("sandbox")).toBe("allow-scripts");
    expect(container.querySelector('button[aria-pressed]')).toBeNull();
    expect(container.textContent).toContain("Download HTML");
    await render(4, [], pdf);
    await click("Open preview");
    expect(container.querySelector('button[aria-pressed]')).toBeNull();
    expect(container.querySelector("iframe")?.src).toContain("/media/w");
    expect(container.textContent).toContain("Download PDF");
    await click("Expand document");
    expect(container.querySelector('[data-testid="conversation"]')?.parentElement?.className).toContain("hidden");
    await click("Close preview");
    expect(container.querySelector("iframe")).toBeNull();
    expect(container.querySelector('[data-testid="conversation"]')?.parentElement?.className).not.toContain("hidden");
  } finally {
    await act(() => root.unmount());
    read.mockRestore(); url.mockRestore(); opened.mockRestore();
    useApp.setState({ connection: before });
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
