# Engineering log

Newest entry first. Record what was done, what broke, and what was learned; decisions
with lasting effect go to `decisions.md`.

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
