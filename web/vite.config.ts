import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig, type Plugin } from "vitest/config";

// In dev/preview the server sets the isolation headers itself. On static hosting the
// vendored coi-serviceworker.js (see justfile `coi`) injects the same headers.
const crossOriginIsolation = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

// coi-serviceworker.js must stay a plain, unbundled, same-origin script. Injecting the tag
// here (instead of writing it in index.html) keeps Vite from trying to bundle it.
const coiServiceWorkerTag: Plugin = {
  name: "lvp:coi-serviceworker-tag",
  transformIndexHtml: () => [
    { tag: "script", attrs: { src: "coi-serviceworker.js" }, injectTo: "head-prepend" },
  ],
};

export default defineConfig({
  // Relative URLs so the bundle works from any sub-path (e.g. GitHub Pages).
  base: "./",
  plugins: [svelte(), coiServiceWorkerTag],
  server: { headers: crossOriginIsolation },
  preview: { headers: crossOriginIsolation },
  build: {
    target: "es2022",
    sourcemap: true,
  },
  worker: { format: "es" },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
