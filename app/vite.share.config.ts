// The reviewer page of a design share (story 51): the app's canvas components in a build of
// their own, served by the share service (chauffeur-design-share). Nothing of the app's daemon
// connection is in it. `bun run build:share` writes dist-share/ (index.html, assets/).

import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@chauffeur/protocol": resolve(import.meta.dirname, "../protocol-ts/src/index.ts"),
      "@": resolve(import.meta.dirname, "src"),
    },
  },
  clearScreen: false,
  server: {
    port: Number(process.env.CHAUFFEUR_VITE_PORT) || 1421,
    strictPort: true,
    fs: { allow: [resolve(import.meta.dirname, "..")] },
    // Development: the share service on loopback answers the page's calls.
    proxy: {
      "/v1": { target: process.env.SHARE_SERVICE || "http://127.0.0.1:8090", ws: true },
    },
  },
  // The app's public/ (its icon, the Office's art) is not the page's.
  publicDir: false,
  envPrefix: ["VITE_"],
  build: {
    target: ["safari16", "chrome110"],
    outDir: "dist-share",
    emptyOutDir: true,
    chunkSizeWarningLimit: 2000,
    rollupOptions: { input: resolve(import.meta.dirname, "share.html") },
  },
});
