// The extension UI SDK, version 1: the modules a separately built extension UI shares with Core.
//
// An extension's `extension.js` bundles its own dependencies but never these: it reads them from
// `globalThis[SDK_GLOBAL]` at run time, so React, ReactDOM and CodeMirror have one instance (hooks,
// contexts and editor facets only work within one) and Core's stores and components are the
// running app's own. Core's bridge (`src/extensions/sdk.ts`) provides exactly this list; a Core
// release that removes or changes one of them in an incompatible way raises `SDK_VERSION`, and
// releases declare the version they were built for (`ui_api`).
//
// Plain TypeScript without enums or parameter properties: the extension build loads this file
// straight from a Core checkout with Node's type stripping.

export const SDK_VERSION = 1;

/** Where Core's bridge lives on the page. */
export const SDK_GLOBAL = `__CHAUFFEUR_SDK_${SDK_VERSION}__`;

/** Third-party modules whose single instance belongs to Core. */
export const SHARED_PACKAGES = [
  "react",
  "react/jsx-runtime",
  "react-dom",
  "react-dom/client",
  "codemirror",
  "@codemirror/commands",
  "@codemirror/lang-css",
  "@codemirror/lang-html",
  "@codemirror/lang-javascript",
  "@codemirror/lang-json",
  "@codemirror/lang-markdown",
  "@codemirror/lang-python",
  "@codemirror/lang-rust",
  "@codemirror/lang-sql",
  "@codemirror/lang-xml",
  "@codemirror/lang-yaml",
  "@codemirror/language",
  "@codemirror/search",
  "@codemirror/state",
  "@codemirror/view",
  "@lezer/highlight",
] as const;

/** Core's own modules an extension may use (`@chauffeur/core/<path>`), and the wire protocol. */
export const SHARED_CORE = [
  "@chauffeur/protocol",
  "@chauffeur/core/App",
  "@chauffeur/core/components/ArchiveDialog",
  "@chauffeur/core/components/ErrorBoundary",
  "@chauffeur/core/components/Markdown",
  "@chauffeur/core/components/chat/ChatPanel",
  "@chauffeur/core/components/ui",
  "@chauffeur/core/extensions/sdk",
  "@chauffeur/core/lib/audioHold",
  "@chauffeur/core/lib/browserOffice",
  "@chauffeur/core/lib/designs",
  "@chauffeur/core/lib/editorTheme",
  "@chauffeur/core/lib/format",
  "@chauffeur/core/lib/keys",
  "@chauffeur/core/lib/models",
  "@chauffeur/core/lib/native",
  "@chauffeur/core/lib/rpc",
  "@chauffeur/core/lib/share",
  "@chauffeur/core/lib/tabDrag",
  "@chauffeur/core/state/app",
  "@chauffeur/core/state/session",
] as const;

export const SHARED_MODULES: readonly string[] = [...SHARED_PACKAGES, ...SHARED_CORE];

/** A module the extension takes from Core. */
export function isShared(specifier: string): boolean {
  return SHARED_MODULES.includes(specifier);
}

/**
 * A module that only works as Core's single instance but that Core does not share: bundling a
 * copy would split React's or CodeMirror's state, so the build refuses it.
 */
export function mustBeShared(specifier: string): boolean {
  return (
    !isShared(specifier) &&
    (specifier === "react" ||
      specifier.startsWith("react/") ||
      specifier === "react-dom" ||
      specifier.startsWith("react-dom/") ||
      specifier === "codemirror" ||
      specifier.startsWith("@codemirror/") ||
      specifier.startsWith("@lezer/") ||
      specifier.startsWith("@chauffeur/"))
  );
}
