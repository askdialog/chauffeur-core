import { expect, spyOn, test } from "bun:test";
import { Window } from "happy-dom";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { SharedDocument } from "./SharedDocument";
import { api, ServiceError } from "./api";

test("shared HTML is isolated, PDFs render, updates replace blobs and revoked access removes the document", async () => {
  const browser = new Window();
  const globals = { window: browser, document: browser.document, IS_REACT_ACT_ENVIRONMENT: true };
  const previous = new Map(Object.keys(globals).map((key) => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  let failure = false;
  let html = "<!doctype html><h1>Résumé</h1>";
  const read = spyOn(api, "documentFile").mockImplementation(async (_id, name) => {
    if (failure) throw new ServiceError(410, "Sharing stopped");
    return new Blob([name.endsWith(".pdf") ? "%PDF-1.4" : html], { type: name.endsWith(".pdf") ? "application/pdf" : "text/html" });
  });
  const resources: Blob[] = [];
  const create = spyOn(URL, "createObjectURL").mockImplementation((blob) => { if (!(blob instanceof Blob)) throw new Error("Expected a document blob"); resources.push(blob); return `blob:test-${resources.length}`; });
  const revoke = spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  let refused = "";
  const onRefused = (text: string) => { refused = text; };
  const onSignedOut = () => {};
  const render = (name: string, revision: number) => act(async () => {
    root.render(<SharedDocument id="share" name={name} title="Report" revision={revision} onSignedOut={onSignedOut} onRefused={onRefused} />);
  });
  try {
    await render("report.html", 0);
    expect(container.querySelector("iframe")?.getAttribute("sandbox")).toBe("allow-scripts");
    expect(await resources[0].text()).toBe(html);
    expect(await resources[1].text()).toContain("default-src 'none'");
    expect(container.querySelector("a")?.getAttribute("href")).toBe("blob:test-1");
    expect(await resources[1].text()).toStartWith('<!doctype html><meta charset="utf-8">');
    expect(resources[1].type).toBe("text/html;charset=utf-8");
    const frame = container.querySelector("iframe");
    await render("report.html", 1);
    expect(container.querySelector("iframe")).toBe(frame);
    expect(resources.length).toBe(2);
    html += "<p>Updated</p>";
    await render("report.html", 2);
    expect(revoke.mock.calls.map(([url]) => url)).toContain("blob:test-2");
    await render("report.pdf", 3);
    expect(container.querySelector("iframe")?.hasAttribute("sandbox")).toBe(false);
    expect(container.querySelector("iframe")?.getAttribute("src")).toContain("#navpanes=0");
    failure = true;
    await render("report.pdf", 4);
    expect(refused).toBe("Sharing stopped");
    expect(container.querySelector("iframe")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
  } finally {
    await act(() => root.unmount());
    read.mockRestore(); create.mockRestore(); revoke.mockRestore();
    await browser.happyDOM.close();
    for (const [key, descriptor] of previous) {
      if (descriptor) Object.defineProperty(globalThis, key, descriptor);
      else Reflect.deleteProperty(globalThis, key);
    }
  }
});
