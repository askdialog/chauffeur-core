import { expect, test } from "bun:test";
import { Window } from "happy-dom";
import { act, StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ReturnToReview } from "./mobileAuth";
import { browserReviewUrl, mobileAuthContext } from "./mobileAuthContext";

test("browser continuation drops native proof while retaining the exact screen and thread", () => {
  const url = new URL(browserReviewUrl("https://design.chauffeur.askdialog.com/s/abcdef01?mobile-auth=1&challenge=abc&state=secret&native=1&mobile=1&screen=home.html&thread=thread1#sign-in=token"));
  expect(url.search).toBe("?screen=home.html&thread=thread1"); expect(url.hash).toBe("");
  expect(mobileAuthContext("?mobile-auth=1&challenge=" + "a".repeat(43) + "&state=" + "b".repeat(64))).not.toBeNull();
  expect(mobileAuthContext("?mobile-auth=1&challenge=" + "a".repeat(43) + "&state=" + "b".repeat(64) + "&state=other")).toBeNull();
});

test("a signed-in mailed continuation never authorizes before explicit account and design consent", async () => {
  const browser = new Window({ url: "https://design.chauffeur.askdialog.com/s/abcdef01?mobile-auth=1&challenge=" + "a".repeat(43) + "&state=" + "b".repeat(64) + "&screen=home.html&thread=thread1" });
  const calls: { url: string; init?: RequestInit }[] = [];
  const globals = { window: browser, document: browser.document, location: browser.location, IS_REACT_ACT_ENVIRONMENT: true, fetch: async (url: RequestInfo | URL, init?: RequestInit) => { calls.push({ url: String(url), init }); return Response.json({ callback: "chauffeur://review/auth?code=" + "c".repeat(64) + "&state=" + "b".repeat(64) }); } };
  const previous = new Map(Object.keys(globals).map(key => [key, Object.getOwnPropertyDescriptor(globalThis, key)]));
  for (const [key, value] of Object.entries(globals)) Object.defineProperty(globalThis, key, { configurable: true, value });
  const container = document.createElement("div"); document.body.append(container); const root = createRoot(container);
  try {
    await act(() => root.render(<StrictMode><ReturnToReview id="abcdef01" title="Actual hosted design" me={{ email: "reviewer@example.com", name: "Reviewer" }} /></StrictMode>));
    expect(calls).toHaveLength(0);
    expect(container.textContent).toContain("Actual hosted design"); expect(container.textContent).toContain("reviewer@example.com");
    expect(container.querySelector('a[href*="screen=home.html"]')?.getAttribute("href")).toBe("https://design.chauffeur.askdialog.com/s/abcdef01?screen=home.html&thread=thread1");
    const button = [...container.querySelectorAll("button")].find(item => item.textContent === "Allow and open Chauffeur"); if (!button) throw Error("Consent action missing");
    await act(async () => { button.click(); await Promise.resolve(); });
    expect(calls).toHaveLength(1); expect(calls[0].url).toBe("/v1/shares/abcdef01/mobile-authorize"); expect(calls[0].init?.headers).toEqual({ "content-type": "application/json", "X-Chauffeur-Mobile": "1" });
  } finally {
    await act(() => root.unmount()); await browser.happyDOM.close();
    for (const [key, descriptor] of previous) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key); }
  }
});
