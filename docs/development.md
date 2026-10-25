# Development guide

## Stack

| Layer | Technology | Where |
|---|---|---|
| Geometry core | Rust, pure, host-testable (`proptest`), rayon for parallel paths | `crates/lvp-core` |
| Browser bridge | Rust → shared-memory WASM via `wasm-bindgen` + `wasm-bindgen-rayon` | `crates/lvp-wasm` |
| Fixture generator | Rust binary writing `fixtures/` from the core's DXF writer | `crates/lvp-fixtures` |
| UI / app shell | Svelte 5, TypeScript, Vite, Web Workers | `web/` |
| Toolchain | Nix flake dev shell (date-pinned nightly Rust), `just` task runner | `flake.nix`, `rust-toolchain.toml`, `justfile` |

The core never touches the DOM or wasm APIs; the wasm crate is a thin adapter that
moves flat `f64` buffers. The UI only ever talks to the core through a Web Worker, which
owns the wasm instance and its rayon thread pool.

## Prerequisites

[Nix](https://nixos.org/download) with flakes enabled (Determinate Nix has them on by
default). Nothing else: Rust, Node, `wasm-bindgen`, `wasm-opt` and `just` all come from
the flake. `direnv` users get the shell automatically from `.envrc`. Nix flakes only see
git-tracked files, so `git add` new files before the first `nix develop` after adding them.

Without Nix, `rustup` honours `rust-toolchain.toml` and you need `wasm-bindgen-cli` of
exactly the version in `Cargo.lock`, `binaryen`, Node 24 and `just` on your own.

## Commands

All recipes run inside the dev shell. Either `nix develop` once and then call `just`,
or prefix each call with `nix develop -c`.

| Command | What it does |
|---|---|
| `just build` | One-shot production build: `npm ci`, release wasm + `wasm-opt`, `svelte-check`, Vite build → `web/dist/` |
| `just dev` | Debug wasm, then Vite dev server with hot reload |
| `just test` | `cargo test` (unit + property tests, DXF/Gerber/SVG/PDF/GDSII/IPC-2581 fixtures, presets, path order, export round trips, fixture staleness, saved fuzz inputs), `svelte-check`, `vitest` driving the real wasm through import → evaluate → export |
| `just e2e` | Browser end-to-end tests of `web/dist` in the locally installed Chrome (run `just build` first; see below) |
| `just regression` | Fixture-based `#[ignore]`d tests |
| `just fixtures` | Regenerate `fixtures/` from `crates/lvp-fixtures` |
| `just lint` | `rustfmt --check` and `clippy -D warnings` for host and wasm targets |
| `just wasm [release]` | Rebuild only the wasm + JS glue into `web/src/wasm/` (run after Rust changes while `just dev` is up) |
| `just fuzz <target> [seconds]` | Coverage-guided fuzzing of one reader or the whole pipeline (see Fuzzing below); `just fuzz-all` runs every target |
| `just preview` | Build, then serve `web/dist` locally |
| `just clean` | Remove build outputs |

`web/dist/` is a self-contained static site: deploy it to any static host, no headers
or server configuration required.

## Reproducibility

- Toolchain versions are pinned by `rust-toolchain.toml` (nightly by date) and `flake.lock`.
- Dependencies are pinned by `Cargo.lock` and `web/package-lock.json`.
- The `wasm-bindgen` crate is pinned exactly (`=x.y.z` in `Cargo.toml`) and must match
  `wasm-bindgen-cli` from nixpkgs. `just check-wasm-bindgen` (run automatically by
  `just wasm`) fails loudly if they drift. Bump `flake.lock` and `Cargo.toml` together.
- Fixtures are generated; a test fails if the committed files differ from the generator.

## Debugging in the browser

The store is exposed as `globalThis.lvp` on the page, so the worker can be driven from the
console or from a DevTools-protocol script: `await lvp.worker.call({ kind: "version" })`.
A wasm trap (`RuntimeError`) leaves the instance unusable; the client detects it, replaces
the worker and the store re-imports the sources, with the message shown in the bar. Rust
panic messages appear in the worker console (`console_error_panic_hook`).

## Browser end-to-end tests

`just e2e` runs the Playwright suite in `web/e2e/` against the production bundle, so run
`just build` first. It drives the Chrome installed on the machine (`channel: "chrome"` in
`web/playwright.config.ts`); nothing is downloaded, and Chrome is the one thing the Nix
shell does not provide. The suite starts two servers itself: Vite preview on port 4190
(isolation headers sent by the server) and `web/e2e/static-server.mjs` on port 4191 (no
headers, so the service worker has to provide them). Covered: import → evaluate → export
of a colour-separated PDF, mapping an unmapped layer onto a new slot, a two-group
subtraction with a hatch pass, and the service worker installing, isolating the page and
serving the app offline.

A failure keeps a trace under `web/test-results/`; open it with
`npx playwright show-trace <file>`. `npx playwright test --headed` (in `web/`) shows the
browser. `just test` does not include this suite because it needs a built bundle and a
browser; the DevTools-protocol approach described above remains useful for one-off checks.

## Continuous integration and deployment

`.github/workflows/ci.yml` runs on every push to `main`, on pull requests and on demand.
It installs Nix, enters the same dev shell as above and runs, in order:

1. `just lint`
2. `just test`
3. `just build`
4. `just e2e` (Playwright drives the Google Chrome preinstalled on the Ubuntu runner)

Cargo and npm downloads and the `target/` directory are cached, keyed on `flake.lock`,
`rust-toolchain.toml`, `Cargo.lock` and `web/package-lock.json`. A failed run keeps the
Playwright traces as the `playwright-results` artifact for a week.

After a green run on `main` the `deploy` job publishes `web/dist` to GitHub Pages:
<https://dmitriyne.github.io/laser-vector-prep/>. Pages sends no isolation headers, so the
first visit goes through the coi-serviceworker reload described below; the build uses
relative URLs (`base: "./"`), so the site works under the `/laser-vector-prep/` sub-path.
The repository's Pages source must be "GitHub Actions" (Settings → Pages).

## Fuzzing

`fuzz/` holds coverage-guided [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz)
targets: one per reader (`dxf`, `gerber`, `svg`, `pdf`, `gds`, `ipc`) and `pipeline`
(import → evaluate a preset → export). They all call `lvp_core::fuzz::run`. The crate is
its own workspace because of libFuzzer's instrumentation.

- `just fuzz <target> [seconds]` seeds from `fixtures/` and grows `fuzz/corpus/<target>`
  (not committed). Extra arguments go to libFuzzer, e.g. `-fork=4 -ignore_crashes=1` to
  keep going after a crash. `just fuzz-all [seconds]` runs every target in turn.
- A crash, hang or allocation over 2 GB stops the run and leaves the input in
  `fuzz/artifacts/<target>/`. Minimise it with
  `cd fuzz && cargo fuzz tmin --sanitizer none -O <target> <file>`, fix the bug, and save
  the input under `fixtures/fuzz/<target>/` with a descriptive name. `just test` replays
  every saved input through the same entry point.
- Fuzz builds keep debug assertions and overflow checks for everything except
  cavalier_contours, whose debug-only self-checks fire on near-degenerate geometry that
  its release build handles (decisions D30). That is set in `fuzz/Cargo.toml`, which is
  why the recipes pass `-O`.
- `.github/workflows/fuzz.yml` runs each target for five minutes every night (and on
  demand with a chosen duration), restoring and saving the corpus through the Actions
  cache. A failing target uploads its input as the `fuzz-<target>` artifact.

## Shared-memory wasm and threads

The wasm module uses a shared, imported linear memory and runs rayon on a pool of Web
Workers. The moving parts:

- `.cargo/config.toml` sets, for the wasm32 target only, the target features
  (`atomics`, `bulk-memory`, `mutable-globals`) and the linker flags (`--shared-memory`,
  `--import-memory`, `--max-memory=1 GiB`, TLS symbol exports). Host builds are untouched.
- `just wasm` builds with `-Z build-std=std,panic_abort` so the std has atomics too, runs
  `wasm-bindgen --target web`, and writes a `package.json` into `web/src/wasm/` so
  `wasm-bindgen-rayon`'s pool worker can import the glue.
- `web/src/worker/geometry.worker.ts` calls `init()` then
  `initThreadPool(navigator.hardwareConcurrency)` once; everything that may fan out to
  rayon must run in this worker, never on the main thread.
- Node-side tests (`vitest`) load the wasm but cannot start the pool. Parallel code paths
  keep a sequential branch below a size threshold so they stay testable there.

Shared memory requires cross-origin isolation:

- Dev and preview servers send `Cross-Origin-Opener-Policy: same-origin` and
  `Cross-Origin-Embedder-Policy: require-corp` directly (`web/vite.config.ts`).
- The production bundle ships `coi-serviceworker.js`: a vendored copy of the MIT
  coi-serviceworker (`web/sw/coi-serviceworker.js`) whose worker half additionally
  precaches the whole bundle, so the site works offline after the first load. The Vite
  build injects the asset list and a content-derived cache name and emits the file at the
  bundle root; a plugin injects its `<script>` tag into `index.html` (production builds
  only). On first load it registers itself, adds the headers and reloads the page once.
  It must stay an unbundled, same-origin file. The app waits for `crossOriginIsolated`
  before spawning the worker. The dev server does not register it.

The header of the running app shows core version, thread count, `crossOriginIsolated`
and `SharedArrayBuffer` status so this can be checked at a glance.

## Layout

```
crates/lvp-core/      geometry core, see docs/architecture.md for the module map
crates/lvp-wasm/      wasm-bindgen exports, thread pool entry point
crates/lvp-fixtures/  fixture generator (lib + bin) and staleness test
web/index.html        app shell; coi-serviceworker.js tag injected at build
web/src/main.ts       mounts App.svelte
web/src/App.svelte    layout: profiles bar, sources + layers, preview, properties + groups + export + diagnostics
web/src/core/types.ts TypeScript mirror of the core's serde types
web/src/state/        project.svelte.ts (ProjectStore), profileStore.ts (IndexedDB)
web/src/ui/           one component per panel (sources, profile sources = bindings, layers, properties, groups, export, diagnostics), Preview.svelte for the canvas
web/src/worker/       protocol.ts (messages), geometry.worker.ts, client.ts (promise façade, stale-result guard)
web/src/wasm/         generated by `just wasm` (git-ignored)
web/sw/               vendored coi-serviceworker with offline precache (emitted at build)
web/e2e/              Playwright end-to-end tests (helpers, workflow, offline, static server); config in web/playwright.config.ts
fixtures/             generated DXF fixtures + manifest.json (see fixtures/README.md)
docs/                 documentation (spec, this guide)
docs/engineering/     engineering log, todo, decisions, known defects
```
