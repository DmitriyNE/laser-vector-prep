import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig, type Plugin } from "vitest/config";

// In dev/preview the server sets the isolation headers itself. On static hosting the
// vendored coi-serviceworker.js (web/sw/) injects the same headers and precaches the bundle.
const crossOriginIsolation = {
  "Cross-Origin-Opener-Policy": "same-origin",
  "Cross-Origin-Embedder-Policy": "require-corp",
};

/**
 * Emits `coi-serviceworker.js` at the bundle root with the asset list injected, and adds
 * its (unbundled, same-origin) script tag to index.html. Production builds only: the dev
 * server already sends the headers and must not register a caching worker.
 */
function serviceWorker(): Plugin {
  let isBuild = false;
  return {
    name: "lvp:service-worker",
    configResolved(config) {
      isBuild = config.command === "build";
    },
    transformIndexHtml() {
      return isBuild ? [{ tag: "script", attrs: { src: "coi-serviceworker.js" }, injectTo: "head-prepend" }] : [];
    },
    generateBundle(_options, bundle) {
      const assets = Object.keys(bundle).filter((f) => !f.endsWith(".map") && f !== "index.html");
      const precache = ["./", "./index.html", ...assets.map((f) => `./${f}`)];
      const source = readFileSync(fileURLToPath(new URL("./sw/coi-serviceworker.js", import.meta.url)), "utf8");
      const version = createHash("sha1").update(precache.join("\n")).digest("hex").slice(0, 12);
      this.emitFile({
        type: "asset",
        fileName: "coi-serviceworker.js",
        source: source.replace("__PRECACHE__", JSON.stringify(precache)).replace("__CACHE_NAME__", `lvp-${version}`),
      });
    },
  };
}

export default defineConfig({
  // Relative URLs so the bundle works from any sub-path (e.g. GitHub Pages).
  base: "./",
  plugins: [svelte(), serviceWorker()],
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
