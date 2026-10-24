# Known defects

Open defects first, then fixed ones. One line each; details go in the log or an issue.

| ID | Status | Area | Description | Found | Fixed |
|---|---|---|---|---|---|
| DEF-4 | fixed | build | Debug wasm trapped (`unreachable`) in the standard library's `hypot` during bleed-aware planning: build-std compiled `compiler_builtins` with the dev profile's overflow checks, and its libm relies on wraparound. `just wasm` now disables overflow checks and debug assertions for `compiler_builtins`. Release builds (CI, Pages) were not affected. | 2026-10-25 | 2026-10-25 |
| DEF-3 | fixed | web | On a static host the first, not yet isolated page load started the geometry worker, which fetched the wasm and failed to start its shared-memory pool before the service-worker reload ("SharedArrayBuffer transfer requires self.crossOriginIsolated"). The worker is now spawned on first use. Found by checking the published site. | 2026-10-25 | 2026-10-25 |
| DEF-2 | fixed | web | Picking several files at once imported only the first: the input was reset while the store iterated its live `FileList` across awaits. Found by the Gerber pair e2e test. | 2026-10-24 | 2026-10-24 |
| DEF-1 | fixed | web | Choosing a built-in preset in the profiles bar did nothing: `structuredClone` of a reactive `$state` proxy throws; needs `$state.snapshot` first. Not covered by unit tests, found by the presets e2e test. | 2026-10-24 | 2026-10-24 |
