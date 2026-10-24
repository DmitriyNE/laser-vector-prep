# Engineering log

Newest entry first. Record what was done, what broke, and what was learned; decisions
with lasting effect go to `decisions.md`.

## 2026-10-24 — Staging follow-up: Svelte, wasm threads, fixtures, license

Owner answered the staging questions; applied all four (D8–D11).

- Toolchain moved to a date-pinned nightly through rust-overlay so the wasm32 std can be
  rebuilt with atomics. Shared-memory wasm now works: the geometry worker starts a
  `wasm-bindgen-rayon` pool; headless Chrome reports 10 threads on this machine and a
  200k-point parallel bounding box comes back correct through both the header-served
  and the coi-serviceworker paths.
- Svelte 5 shell (`App.svelte`) with `svelte-check`; the worker protocol gained
  `threads` and `bbox` requests.
- New core modules: `arc` (DXF bulge and angle semantics, proptested), `dxf::write`
  (R2000 writer with bounds computation), parallel `BBox::of_points_par`.
- `crates/lvp-fixtures` generates five synthetic DXF fixtures plus `manifest.json`;
  a test fails when the committed files are stale. ezdxf audits all five clean and its
  independent reading matches the manifest's entity counts and bounds exactly.
- Apache-2.0 license added.

Learned:

- Current nightlies do not add `--shared-memory`/`--import-memory`/TLS exports to the
  wasm link when `atomics` is on; without them wasm-bindgen fails with "threading
  requires memory to be imported" and then "failed to find `__wasm_init_tls`".
- `wasm-bindgen-rayon`'s `no-bundler` helper is wrong for Vite: it fetches
  `import.meta.url` as a blob and re-runs the whole chunk it was inlined into, which
  surfaced as "#<Memory> could not be cloned" and ten "Invalid URL" errors. The default
  helper plus a `package.json` in the generated directory is what Vite wants.
- The generated glue references `self` at import time; Node tests shim it with an inert
  `EventTarget` and never start the pool.
- svelte-check 4 refuses TypeScript 7 (peer range `^5 || ^6`).

## 2026-10-24 — Staging

Scaffolded the repository against spec v1.3.

- Nix flake dev shell (nixpkgs-unstable): rustc 1.99, wasm-bindgen-cli 0.2.129,
  binaryen 132, Node 24, just. No overlay needed (D1).
- Cargo workspace: `lvp-core` (pure geometry, proptests) and `lvp-wasm` (bindings).
  First real core pieces: `Point`/`BBox`, `Unit` conversions, bounded-error arc
  flattening (D6), each with property tests.
- Vite 8 + TypeScript 7 web shell. Geometry runs in a module Web Worker behind a small
  promise façade with request ids, so stale results can never resolve newer calls.
  `vitest` loads the real wasm in Node.
- Cross-origin isolation via headers in dev and `coi-serviceworker` in the static
  bundle (D5).
- `justfile`: `build`, `dev`, `test`, `regression`, `lint`, `wasm`, `coi`, `preview`,
  `clean`; `just check-wasm-bindgen` guards the CLI/crate version pin (D2).
- Verified the built bundle in headless Chrome (DevTools protocol): served with COOP/COEP
  headers, the worker answers with the core version and `crossOriginIsolated` is true;
  served by a plain static server, coi-serviceworker registers, reloads once, and the
  page ends up isolated with the service worker controlling it. Not yet automated
  (Playwright e2e is an M1 todo).

Learned:

- Nix flakes only see git-tracked files, so new files must be `git add`ed before the
  first `nix develop`.
- nixpkgs' rustc has no bundled `rust-lld`; wasm32 linking needs `lld` in the shell.
- A proptest found that `BBox::expanded(m).expanded(-m)` on a single-point box can
  invert by one ulp. Resolved with an explicit noise floor (D6) instead of weakening
  the test.
- TypeScript cannot infer a generic through `Omit<…>`; the worker protocol is a
  payload union plus `Request = Payload & { id }` so call sites infer the result type.
