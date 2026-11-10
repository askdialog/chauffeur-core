import { afterEach, expect, test } from "bun:test";
import type { EventEnvelope, ExtensionId, ExtensionUi } from "@chauffeur/protocol";
import type { LeaseState } from "@/lib/rpc";
import {
  assetUrl,
  extensionUi,
  holdLease,
  loadExtensionUi,
  packageUrl,
  resetRegistry,
  setRegistryClient,
  setUiLoader,
  type ExtensionContribution,
  type ExtensionHost,
} from "./registry";

afterEach(resetRegistry);

const BASE = "http://127.0.0.1:4100/extension-assets/database/";
const ui = (revision: string, stylesheet: string | null = "ui/extension.css"): ExtensionUi => ({
  revision,
  base_url: `${BASE}${revision}/nonce/`,
  script: "ui/extension.js",
  stylesheet,
  assets: "ui/assets",
});

/** A page in memory: which tags are in it, and what each revision's script registers. */
function page(scripts: Record<string, (host: ExtensionHost) => ExtensionContribution>, failStylesheet = false) {
  const present = new Set<string>();
  setUiLoader({
    stylesheet: async (_id, href) => {
      if (failStylesheet) throw new Error("stylesheet failed");
      present.add(href);
      return { remove: () => present.delete(href) };
    },
    script: async (_id, src, host) => {
      const run = scripts[host.revision];
      if (!run) throw new Error(`no script at ${src}`);
      const contribution = run(host);
      present.add(src);
      return { element: { remove: () => present.delete(src) }, contribution };
    },
  });
  return present;
}

test("a revision's URLs stay beneath its package", () => {
  const metadata = ui("abc");
  expect(packageUrl(metadata, metadata.script)).toBe(`${BASE}abc/nonce/ui/extension.js`);
  expect(assetUrl(metadata, "office/garden.glb")).toBe(`${BASE}abc/nonce/ui/assets/office/garden.glb`);
  expect(() => packageUrl(metadata, "../other/extension.js")).toThrow("outside its package");
  expect(() => packageUrl(metadata, "https://example.com/x.js")).toThrow("outside its package");
  expect(() => assetUrl(metadata, "office/../../../engine")).toThrow("outside its package");
  // A path base_url (the browser listener's) resolves against the page.
  expect(packageUrl({ base_url: "/extension-assets/office/s/n/" }, "ui/extension.js", "http://127.0.0.1:9/#office")).toBe(
    "http://127.0.0.1:9/extension-assets/office/s/n/ui/extension.js",
  );
});

test("a new revision replaces the loaded one only once its stylesheet and script loaded", async () => {
  const one = () => null;
  const two = () => null;
  const present = page({ r1: () => ({ workspace: one }), r2: () => ({ workspace: two }) });
  await loadExtensionUi("database", ui("r1"));
  expect(extensionUi("database").loaded?.contribution.workspace).toBe(one);
  await loadExtensionUi("database", ui("r2"));
  expect(extensionUi("database").loaded?.revision).toBe("r2");
  expect(extensionUi("database").loaded?.contribution.workspace).toBe(two);
  // The previous revision's tags are gone, the new one's are there.
  expect([...present].sort()).toEqual([`${BASE}r2/nonce/ui/extension.css`, `${BASE}r2/nonce/ui/extension.js`]);
});

test("a revision that fails to load leaves the previous one whole and says why", async () => {
  const one = () => null;
  const present = page({
    r1: () => ({ workspace: one }),
    r2: () => {
      throw new Error("boom");
    },
  });
  await loadExtensionUi("database", ui("r1"));
  await expect(loadExtensionUi("database", ui("r2"))).rejects.toThrow("boom");
  const state = extensionUi("database");
  expect(state.loaded?.revision).toBe("r1");
  expect(state.loaded?.contribution.workspace).toBe(one);
  expect(state.failed).toEqual({ revision: "r2", error: "boom" });
  expect([...present].sort()).toEqual([`${BASE}r1/nonce/ui/extension.css`, `${BASE}r1/nonce/ui/extension.js`]);
});

test("a stylesheet that does not load keeps the script from running", async () => {
  let ran = false;
  page({ r1: () => ((ran = true), {}) }, true);
  await expect(loadExtensionUi("database", ui("r1"))).rejects.toThrow("stylesheet failed");
  expect(ran).toBe(false);
  expect(extensionUi("database").loaded).toBeNull();
});

test("loading the loaded revision again runs nothing", async () => {
  let runs = 0;
  page({ r1: () => ((runs += 1), {}) });
  await Promise.all([loadExtensionUi("database", ui("r1")), loadExtensionUi("database", ui("r1"))]);
  await loadExtensionUi("database", ui("r1"));
  expect(runs).toBe(1);
});

test("the host gives the revision's assets and calls the extension's engine", async () => {
  const calls: unknown[] = [];
  setRegistryClient({
    call: async (method, params, workspace, id) => (calls.push({ method, params, workspace, id }), "ok"),
    watch: () => ({ state: { held: false, error: null }, retry() {}, close() {} }),
  });
  let host: ExtensionHost | null = null;
  page({ r1: (given) => ((host = given), {}) });
  await loadExtensionUi("office" as ExtensionId, { ...ui("r1"), base_url: "http://127.0.0.1:4100/extension-assets/office/r1/n/" });
  expect(host!.asset("office/agent.glb")).toBe("http://127.0.0.1:4100/extension-assets/office/r1/n/ui/assets/office/agent.glb");
  expect(await host!.call("office.state", {})).toBe("ok");
  expect(calls).toEqual([{ method: "office.state", params: {}, workspace: null, id: "office" }]);
});

test("views share one lease per extension, given back when the last one unmounts", () => {
  const opened: Array<{ id: ExtensionId; closed: boolean; onEvent: (envelope: EventEnvelope) => void; onState: (state: LeaseState) => void }> = [];
  setRegistryClient({
    call: async () => null,
    watch: (id, onEvent, onState) => {
      const lease = { id, closed: false, onEvent, onState };
      opened.push(lease);
      return { state: { held: false, error: null }, retry() {}, close: () => void (lease.closed = true) };
    },
  });
  const first = holdLease("database");
  const second = holdLease("database");
  expect(opened).toHaveLength(1);
  first();
  first();
  expect(opened[0]!.closed).toBe(false);
  second();
  expect(opened[0]!.closed).toBe(true);
  holdLease("database")();
  expect(opened).toHaveLength(2);
});

test("an extension's events reach its handlers while its lease is held", async () => {
  let deliver: ((envelope: EventEnvelope) => void) | null = null;
  setRegistryClient({
    call: async () => null,
    watch: (_id, onEvent) => ((deliver = onEvent), { state: { held: true }, retry() {}, close() {} }),
  });
  let host: ExtensionHost | null = null;
  page({ r1: (given) => ((host = given), {}) });
  await loadExtensionUi("database", ui("r1"));
  const seen: unknown[] = [];
  const stop = host!.onEvent((event) => seen.push(event));
  const release = holdLease("database");
  deliver!({ seq: null, ts: "t", event: { scope: "extension", id: "database", event: { kind: "progress", n: 1 } } });
  deliver!({ seq: null, ts: "t", event: { scope: "extension", id: "office", event: { kind: "other" } } });
  stop();
  deliver!({ seq: null, ts: "t", event: { scope: "extension", id: "database", event: { kind: "late" } } });
  release();
  expect(seen).toEqual([{ kind: "progress", n: 1 }]);
});
