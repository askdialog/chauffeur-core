// What an extension's UI imports to plug into Core (`@chauffeur/core/extensions/sdk`), SDK 1.
//
//   import { registerExtension } from "@chauffeur/core/extensions/sdk";
//   registerExtension("database", (host) => ({ workspace: DatabasePanel }));
//
// The package's `extension.js` must call it while it runs, once: Core loads it, then shows
// what it registered in the views the user opens.

import type { ExtensionId } from "@chauffeur/protocol";
import { acceptRegistration, currentHost, type ExtensionHost, type ExtensionSetup } from "./registry";

export type { ExtensionContribution, ExtensionHost, ExtensionSetup } from "./registry";

export function registerExtension(id: ExtensionId, setup: ExtensionSetup) {
  acceptRegistration(id, setup);
}

/**
 * The host of the extension being loaded, for a module that needs it while the script runs
 * (an asset URL computed at import time); null once loading is over: keep what `setup` gets.
 */
export function loadingHost(): ExtensionHost | null {
  return currentHost();
}
