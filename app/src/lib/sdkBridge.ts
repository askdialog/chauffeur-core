// The object Core puts on the page for separately built extension UIs (app/sdk/modules.ts):
// each shared module as the module object their bundles expect.

import { SDK_VERSION } from "../../sdk/modules";

/**
 * The module object an extension's bundle gets: live bindings of the namespace, marked as an ES
 * module so that its default import is the module's default export.
 */
export function sdkModule(namespace: object): object {
  const module = Object.create(null) as Record<string, unknown>;
  Object.defineProperty(module, "__esModule", { value: true });
  for (const key of Object.keys(namespace)) {
    Object.defineProperty(module, key, { enumerable: true, get: () => (namespace as Record<string, unknown>)[key] });
  }
  return Object.freeze(module);
}

export type SdkBridge = {
  readonly version: number;
  readonly modules: readonly string[];
  require(specifier: string): object;
};

export function makeBridge(shared: Record<string, object>): SdkBridge {
  const modules = new Map(Object.entries(shared).map(([specifier, namespace]) => [specifier, sdkModule(namespace)]));
  return Object.freeze({
    version: SDK_VERSION,
    modules: Object.freeze([...modules.keys()]),
    require(specifier: string) {
      const module = modules.get(specifier);
      if (!module) throw new Error(`Chauffeur's extension SDK ${SDK_VERSION} has no module ${specifier}`);
      return module;
    },
  });
}
