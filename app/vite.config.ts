import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

/** The daemon's state folder, as the daemon resolves it. */
function chauffeurHome(): string {
  return process.env.CHAUFFEUR_HOME || join(homedir(), ".chauffeur");
}

/** `{url, token}` of the running daemon, read fresh from `daemon.json` and `token`. */
function readConnection(): { url: string; token: string } | null {
  try {
    const home = chauffeurHome();
    const info = JSON.parse(readFileSync(join(home, "daemon.json"), "utf8")) as { port: number };
    const token = readFileSync(join(home, "token"), "utf8").trim();
    return { url: `ws://127.0.0.1:${info.port}/ws`, token };
  } catch {
    return null;
  }
}

/**
 * Development only: lets the UI run in a plain browser, without Tauri, against the daemon of
 * `$CHAUFFEUR_HOME`. The connection is injected into index.html and also served at
 * `/__chauffeur/connection` so that a reconnect picks up a restarted daemon's new port.
 */
function devDaemonConnection(): Plugin {
  return {
    name: "chauffeur-dev-daemon-connection",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__chauffeur/connection", (req, res) => {
        // The token is for the app's own page: a page on another localhost port gets nothing
        // (browsers send Sec-Fetch-Site; a non-browser client sends neither header).
        const site = req.headers["sec-fetch-site"];
        if (req.headers.origin !== undefined || (site !== undefined && site !== "same-origin" && site !== "none")) {
          res.statusCode = 403;
          res.end();
          return;
        }
        const connection = readConnection();
        res.setHeader("content-type", "application/json");
        res.setHeader("cache-control", "no-store");
        res.statusCode = connection ? 200 : 503;
        res.end(
          JSON.stringify(
            connection ?? { error: `no daemon.json or token in ${chauffeurHome()}; start chauffeurd` },
          ),
        );
      });
    },
    transformIndexHtml() {
      const connection = readConnection();
      if (!connection) return [];
      return [
        {
          tag: "script",
          injectTo: "head-prepend",
          children: `window.__CHAUFFEUR_DEV_CONNECTION__ = ${JSON.stringify(connection)};`,
        },
      ];
    },
  };
}

export default defineConfig({
  plugins: [react(), tailwindcss(), devDaemonConnection()],
  resolve: {
    dedupe: ["react", "react-dom", "@codemirror/state", "@codemirror/view"],
    alias: {
      "@chauffeur/core": resolve(import.meta.dirname, "src"),
      "@chauffeur/protocol": resolve(import.meta.dirname, "../protocol-ts/src/index.ts"),
      "@": resolve(import.meta.dirname, "src"),
    },
  },
  clearScreen: false,
  server: {
    port: Number(process.env.CHAUFFEUR_VITE_PORT) || 1420,
    strictPort: true,
    // The page embeds the daemon's token: no other origin (another dev server on localhost,
    // which Vite would allow by default) may read it.
    cors: false,
    fs: { allow: [resolve(import.meta.dirname, "..")] },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: {
    target: "safari16",
    copyPublicDir: true,
    outDir: "dist",
    emptyOutDir: true,
    chunkSizeWarningLimit: 2000,
  },
});
