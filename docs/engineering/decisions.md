# Decisions

Short records of choices that are not obvious from the code. Add a new numbered entry
rather than rewriting an old one; mark superseded entries as such.

## D1 — Toolchain from nixpkgs only, no Rust overlay (2026-10-24) — superseded by D8

nixpkgs' `rustc` bundles the `wasm32-unknown-unknown` std, so a single `nixpkgs-unstable`
input was enough for a single-threaded wasm build. Superseded the same day when the owner
asked for shared-memory wasm, which needs `-Z build-std` and therefore nightly (D8).

## D2 — `wasm-bindgen` crate pinned exactly to the nixpkgs CLI (2026-10-24)

`wasm-bindgen-cli` and the `wasm-bindgen` crate must match exactly. We pin
`wasm-bindgen = "=0.2.129"` in `Cargo.toml` and let `just check-wasm-bindgen` fail the
build on drift. `wasm-pack` was rejected because it downloads a CLI binary at build time.

## D3 — Reproducibility via pinned shell + lockfiles; no `nix build` package yet (2026-10-24)

`nix develop -c just build` is the one-command build. A full `nix build` derivation
would add `npmDepsHash`/cargo-vendoring maintenance for little gain at this stage.
Can be added later without changing the layout.

## D4 — Vanilla TypeScript for the staging shell; UI framework undecided (2026-10-24) — superseded by D9

Deferred the framework choice to the owner; answered the same day: Svelte (D9).

## D5 — Cross-origin isolation: headers in dev, coi-serviceworker in production (2026-10-24)

Vite dev/preview servers send COOP/COEP headers directly. The static bundle uses
`coi-serviceworker.js` (npm package, copied to `web/public/` by `just coi`, never
bundled). Isolation is required, not optional: the wasm module uses shared memory (D8),
so the app does not even spawn the geometry worker until `crossOriginIsolated` is true
(the service worker reloads the page once on first visit to get there).

## D6 — Numerical policy: explicit tolerances, 1 pm noise floor (2026-10-24)

Arcs are flattened to polylines with vertices exactly on the curve, chordal deviation
(sagitta) ≤ caller tolerance, and no segment spanning more than a quarter turn. The
segment count is the minimum satisfying both. Endpoints are exact (interpolated on the
angle, not accumulated). Tolerance is always an explicit parameter in mm; there is no
global default in the core.

`geom::EPS_MM = 1e-9` mm is the floating-point noise floor: differences below it are
snapped, not diagnosed (found by proptest: shrinking a degenerate box by exactly its
expansion margin inverts it by one ulp). Anything above it is a real geometric event
and is reported to the caller. The integer scaling for the boolean kernel (D7) must be
chosen so its resolution is well above this floor.

## D8 — Date-pinned nightly via rust-overlay; shared-memory wasm with rayon (2026-10-24)

The owner wants shared memory and threads from the start. That needs the wasm32 std
rebuilt with atomics (`-Z build-std=std,panic_abort`), hence nightly. `rust-toolchain.toml`
pins `nightly-2026-10-24` and is read by both rustup and the flake
(`rust-bin.fromRustupToolchainFile`), so there is one source of truth for the toolchain.
The nightly bundles `rust-lld`; no separate `lld`.

Linker flags live in `.cargo/config.toml` under `[target.wasm32-unknown-unknown]` only, so
host builds and tests stay ordinary. Current nightlies no longer add the shared-memory
linker flags when `atomics` is enabled, so we pass them ourselves: `--shared-memory`,
`--import-memory`, `--max-memory=1 GiB`, and `--export=` for `__wasm_init_tls`,
`__tls_size`, `__tls_align`, `__tls_base`, which wasm-bindgen's threading transform
requires. Threads come from `wasm-bindgen-rayon` with its default (bundler) helper:
Vite emits the self-spawning pool worker as its own chunk and resolves the glue through a
`package.json` that `just wasm` writes into the generated directory. The `no-bundler`
feature does not work under Vite: its blob self-fetch re-executes whichever chunk the
helper was inlined into.

The pool is created inside the geometry worker (`initThreadPool(hardwareConcurrency)`),
never on the main thread. Node-side tests load the wasm but cannot start the pool, so
anything that fans out to rayon keeps a sequential path below a size threshold.

## D9 — Svelte 5 for the UI (2026-10-24)

Owner's choice. Runes-mode components, `svelte-check` as the type checker (it covers
`.ts` too, so `tsc` is not run separately). svelte-check 4 requires TypeScript ≤ 6, which
pins `typescript` to `^6` for now. The worker protocol and client stay framework-free.

## D10 — Regression fixtures are synthetic and generated (2026-10-24)

The historical pogo-board files are not used (owner decision). Fixtures are produced by
`crates/lvp-fixtures` from the core's own DXF writer, deterministically, with a
`manifest.json` stating what a correct pipeline must observe (bounds in mm, entity counts,
island/hole counts, unit conversions, block transforms). A test fails when the committed
files differ from the generator's output, so fixtures are never hand-edited. The writer
targets DXF R2000 with handles and a full table skeleton; ezdxf audits the output clean and
agrees with the manifest's counts and bounds.

## D11 — License: Apache-2.0 (2026-10-24)

Owner's choice; `LICENSE` at the root, `license` fields in Cargo and npm manifests.

## D7 — Geometry kernel: to be chosen in M1 (open)

Spec prefers Clipper2 WASM. With a Rust core the candidates are: `i_overlay` (pure Rust
booleans, integer-robust, includes offsetting), `cavalier_contours` (polyline offsetting
with true arcs), or Clipper2 C++ via `cxx` (painful on wasm32-unknown-unknown). Decide
with a small benchmark/robustness spike on the pogo-board fixture before M1 contour fill.
