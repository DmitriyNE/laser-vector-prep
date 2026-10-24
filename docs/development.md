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
| `just test` | `cargo test` (unit + property tests, fixture staleness), `svelte-check`, `vitest` against the built wasm |
| `just regression` | Fixture-based `#[ignore]`d tests |
| `just fixtures` | Regenerate `fixtures/` from `crates/lvp-fixtures` |
| `just lint` | `rustfmt --check` and `clippy -D warnings` for host and wasm targets |
| `just wasm [release]` | Rebuild only the wasm + JS glue into `web/src/wasm/` (run after Rust changes while `just dev` is up) |
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
- The production bundle ships `coi-serviceworker.js` (copied from npm by `just coi`,
  injected into `index.html` by a small Vite plugin). On first load it registers a service
  worker that adds the same headers and reloads the page once. It must stay an unbundled,
  same-origin file. The app waits for `crossOriginIsolated` before spawning the worker.

The header of the running app shows core version, thread count, `crossOriginIsolated`
and `SharedArrayBuffer` status so this can be checked at a glance.

## Layout

```
crates/lvp-core/      geometry core: geom, units, flatten, arc, dxf::write + proptests
crates/lvp-wasm/      wasm-bindgen exports, thread pool entry point
crates/lvp-fixtures/  fixture generator (lib + bin) and staleness test
web/index.html        app shell; coi-serviceworker.js tag injected at build
web/src/main.ts       mounts App.svelte
web/src/App.svelte    status header + smoke tests (to be replaced by the real panels)
web/src/worker/       protocol.ts (messages), geometry.worker.ts, client.ts (promise façade, stale-result guard)
web/src/wasm/         generated by `just wasm` (git-ignored)
web/public/           static files copied verbatim into dist
fixtures/             generated DXF fixtures + manifest.json (see fixtures/README.md)
docs/                 documentation (spec, this guide)
docs/engineering/     engineering log, todo, decisions, known defects
```
