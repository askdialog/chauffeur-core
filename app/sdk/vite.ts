// Builds an extension's UI against the SDK (modules.ts): one IIFE `extension.js` with no imports,
// its stylesheet `extension.css` and the files of `public/` under `assets/`.
//
// An extension's `ui/vite.config.ts` adds this plugin before React's and Tailwind's:
//
//   const { chauffeurExtension } = await import(join(core, "app/sdk/vite.ts"));
//   export default defineConfig({ plugins: [chauffeurExtension({ core }), react(), tailwindcss()] });
//
// Type-only imports: the extension's own Vite runs this file, Core's packages are not needed.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative, sep } from "node:path";
import type { Plugin } from "vite";
import { isShared, mustBeShared, SDK_GLOBAL } from "./modules.ts";

const SHARED_PREFIX = "\0chauffeur-sdk:";

export type ExtensionBuildOptions = {
  /** The Core source folder the SDK is read from. */
  core: string;
  /** The registration module, relative to the UI folder. */
  entry?: string;
};

export function chauffeurExtension({ core, entry = "src/registration.tsx" }: ExtensionBuildOptions): Plugin {
  let publicDir: string | null = null;
  return {
    name: "chauffeur-extension",
    enforce: "pre",
    config() {
      return {
        // Bundled libraries test `process.env.NODE_ENV`, which a library build leaves as it is.
        define: { "process.env.NODE_ENV": JSON.stringify("production") },
        resolve: { alias: { "@chauffeur/sdk/theme.css": join(core, "app/sdk/theme.css") } },
        build: {
          target: "safari16",
          outDir: "dist",
          emptyOutDir: true,
          // `public/` lands in `assets/` (generateBundle), the folder a release declares as its assets.
          copyPublicDir: false,
          assetsDir: "assets",
          // A file a stylesheet or a module imports stays a file of the package, never inlined.
          assetsInlineLimit: 0,
          cssCodeSplit: false,
          minify: true,
          sourcemap: false,
          lib: {
            entry,
            formats: ["iife"],
            name: "ChauffeurExtension",
            fileName: () => "extension.js",
            cssFileName: "extension",
          },
        },
      };
    },
    configResolved(config) {
      publicDir = config.publicDir || null;
    },
    generateBundle() {
      // Always there, even empty: the release names it.
      this.emitFile({ type: "asset", fileName: "assets/.keep", source: "" });
      if (!publicDir) return;
      let names: string[];
      try {
        names = readdirSync(publicDir, { recursive: true, encoding: "utf8" });
      } catch {
        return;
      }
      for (const name of names.sort()) {
        const file = join(publicDir, name);
        if (!statSync(file).isFile()) continue;
        const path = relative(publicDir, file).split(sep).join("/");
        this.emitFile({ type: "asset", fileName: `assets/${path}`, source: readFileSync(file) });
      }
    },
    resolveId(source) {
      if (isShared(source)) return `${SHARED_PREFIX}${source}`;
      if (mustBeShared(source)) {
        this.error(
          `${source} must be Core's single instance, which SDK ${SDK_GLOBAL} does not share: ` +
            "import a module of app/sdk/modules.ts instead, or add it to Core's SDK first",
        );
      }
      return null;
    },
    load(id) {
      if (!id.startsWith(SHARED_PREFIX)) return null;
      const specifier = id.slice(SHARED_PREFIX.length);
      // CommonJS, so that the bundler maps named and default imports onto the module object.
      return `module.exports = globalThis.${SDK_GLOBAL}.require(${JSON.stringify(specifier)});`;
    },
  };
}
