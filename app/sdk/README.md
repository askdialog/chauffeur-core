# Extension UI SDK

What a separately built extension UI shares with Core (SDK 1). Official extensions build their
`ui/extension.js` (one IIFE with no imports), `ui/extension.css` and `ui/assets/` against this
folder, then ship them in their own signed packages; the app loads the installed revision at run
time (`src/extensions/registry.ts`).

- `modules.ts`: the modules Core shares at run time through `globalThis.__CHAUFFEUR_SDK_1__`
  (React, ReactDOM, CodeMirror and Lezer, `@chauffeur/protocol`, Core's exported modules). An
  extension never bundles its own copy; the build refuses one. `src/extensions/bridge.ts` provides
  exactly this list (a test holds them together). An incompatible change raises `SDK_VERSION`.
- `vite.ts`: the Vite plugin an extension's `ui/vite.config.ts` adds before React's and Tailwind's.
  It runs under the extension's own Vite with Node's type stripping: no Core packages needed.
- `theme.css`: Core's Tailwind theme tokens. Core's stylesheet emits them all; an extension's
  stylesheet refers to them (`theme(reference)`), so it follows the running Core's theme.

An extension registers what it contributes with `registerExtension` from
`@chauffeur/core/extensions/sdk`, while its script runs.

## Checking a packaged extension in the dev app

Packages bundle their own libraries in production mode. A library that carries React internals
(the Office's `@react-three/fiber` reconciler) only works with a production React, and Vite's dev
server serves React's development build: the Office then fails with
`dispatcher.getOwner is not a function`. The Tauri app is always a production build. To try a
packaged extension in the browser app (verify-in-app skill), start the dev server in production
mode instead of plain `bunx vite`:

```bash
CHAUFFEUR_HOME=$T/home NODE_ENV=production bunx vite --mode production
```

Remove `node_modules/.vite` first when switching modes, so that Vite bundles React again.
