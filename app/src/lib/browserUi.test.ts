import { expect, test } from "bun:test";
import type { ExtensionUi } from "@chauffeur/protocol";
import { fetchBrowserUi, SESSION_ENDED } from "./browserOffice";

const metadata: ExtensionUi = { revision: "r", base_url: "/extension-assets/office/r/n/", script: "ui/extension.js", stylesheet: "ui/extension.css", assets: "ui/assets" };

function answering(status: number, body: unknown) {
  const asked: Array<{ url: string; init: RequestInit | undefined }> = [];
  const fetcher = (async (url: string, init?: RequestInit) => {
    asked.push({ url, init });
    return new Response(typeof body === "string" ? body : JSON.stringify(body), { status });
  }) as unknown as typeof fetch;
  return { asked, fetcher };
}

test("the browser page asks its listener for the installed Office with its session", async () => {
  const { asked, fetcher } = answering(200, metadata);
  expect(await fetchBrowserUi("tok", fetcher)).toEqual(metadata);
  expect(asked[0]!.url).toBe("/extension-ui/office");
  expect(asked[0]!.init?.headers).toEqual({ authorization: "Bearer tok" });
  expect(asked[0]!.init?.cache).toBe("no-store");
});

test("an Office not ready yet is waited for, an ended session is final", async () => {
  expect(await fetchBrowserUi("tok", answering(503, "starting").fetcher)).toBeNull();
  await expect(fetchBrowserUi("tok", answering(401, "").fetcher)).rejects.toThrow(SESSION_ENDED);
  await expect(fetchBrowserUi("tok", answering(404, "no such extension").fetcher)).rejects.toThrow("no such extension");
});
