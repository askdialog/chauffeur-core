import { expect, test } from "bun:test";
import { SHARED_MODULES } from "../../sdk/modules";
import { makeBridge, sdkModule } from "./sdkBridge";

test("an extension's default import is the module's default export, its named imports live", () => {
  const namespace = { default: "the default", value: 1 } as Record<string, unknown>;
  const module = sdkModule(namespace) as Record<string, unknown>;
  expect(module.__esModule).toBe(true);
  expect(module.default).toBe("the default");
  namespace.value = 2;
  expect(module.value).toBe(2);
  expect(Object.isFrozen(module)).toBe(true);
});

test("the bridge gives only the modules it shares, and says which is missing", () => {
  const bridge = makeBridge({ react: { useState: 1 } });
  expect((bridge.require("react") as { useState: number }).useState).toBe(1);
  expect(() => bridge.require("three")).toThrow("has no module three");
  expect(bridge.modules).toEqual(["react"]);
});

test("Core's bridge shares exactly the modules the SDK declares", async () => {
  const source = await Bun.file(new URL("../extensions/bridge.ts", import.meta.url)).text();
  const table = source.slice(source.indexOf("const namespaces"), source.indexOf("};", source.indexOf("const namespaces")));
  const bridged = [...table.matchAll(/^ {2}(?:"([^"]+)"|([a-zA-Z]+))[:,]/gm)].map((match) => match[1] ?? match[2]);
  expect(bridged.sort()).toEqual([...SHARED_MODULES].sort());
});

test("the app loads the bridge with the first extension, not at start", async () => {
  // The bridge imports every shared module, CodeMirror's languages included: kept out of the
  // entry chunk so that an app showing no extension does not load them.
  const main = await Bun.file(new URL("../main.tsx", import.meta.url)).text();
  const registry = await Bun.file(new URL("../extensions/registry.ts", import.meta.url)).text();
  expect(main).not.toMatch(/from "\.\/extensions\/bridge"/);
  expect(registry).not.toMatch(/from "\.\/bridge"/);
  expect(registry).toContain('import("./bridge")');
});
