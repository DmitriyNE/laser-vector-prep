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

Amended for offline support (M1): only one service worker can control the scope, so the
coi-serviceworker is vendored (`web/sw/coi-serviceworker.js`, MIT) and its worker half
extended with a cache-first precache of the bundle; the Vite build injects the asset list.
No workbox or PWA plugin.

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

## D7 — Geometry kernel: cavalier_contours (2026-10-24, owner decision)

Candidates were spiked against fixture-shaped inputs: `i_overlay` 9.1 (pure Rust, integer
arithmetic internally, all booleans and fill rules, offsets with bevel/miter/round joins,
stroke expansion, line clipping; contour-fill loop on the 20×20 mm hole fixture at 0.5 mm
pitch in 1.6 ms, union of 5000 random rectangles in 90 ms), `cavalier_contours` 0.9
(line+arc polylines with arcs preserved through offsets, pairwise closed-polyline
union/intersection/difference, shape offsets with islands, segment intersection
primitives), and Clipper2 via C++ (rejected: needs a C++ wasm toolchain incompatible with
our `build-std` flow; the Rust `clipper2` crate wraps the C++ via `cc`).

The owner chose `cavalier_contours` for arc fidelity: contour rings around round pads stay
true arcs instead of flattened chords. Consequences we accept and own:

- Multi-shape booleans (union of many primitives, XOR, clipping) are built from pairwise
  `boolean()` calls driven by a spatial index; XOR = (A∪B) − (A∩B).
- Hatch is our scanline intersection against line and arc segments (exact endpoints on
  arcs), not a kernel clip.
- Stroke expansion of linework is a union of per-segment capsules (arcs are native).
- Gap fill uses a Voronoi medial axis (`boostvoronoi`) of segment-approximated slivers.
- Each segment's |bulge| must be ≤ 1 (half circle); importers split larger arcs.

`i_overlay` stays the documented fallback if pairwise booleans turn out too slow on
flash-heavy inputs; the spike numbers above are the yardstick.

## D12 — The Rust core owns the project model (2026-10-24)

Parsed sources, the working-layer DAG with its evaluation cache, pattern passes, the
route planner, exporters, and the versioned profile schema with migrations all live in
`lvp-core`. The worker holds one `Project`; TypeScript keeps UI state plus a JSON mirror
of the configuration, sends whole-config updates, and receives diagnostics and preview
buffers tagged with a revision. The spec's acceptance tests become cargo tests.

## D13 — Fill strategies are our own code on top of kernel primitives (2026-10-24)

No library implements the spec's fill semantics (pitch, first offset, phase in document
mm, edge coverage, hole handling). The kernel supplies offsets, booleans and intersection
primitives; `lvp-core::patterns` implements inward contour fill, gap fill, passthrough,
boundary trace, parallel/cross hatch and boundary+hatch, each proptested in mm.

## D14 — DXF parsing: own parser (2026-10-24)

Owner initially chose the `dxf` crate (0.6.2). Investigation: its entity set is generated
from a spec in which HATCH is commented out, so hatches cannot be represented; it also
pulls `image`, `chrono`, `encoding_rs` and `uuid` (needs the `js` feature) into the wasm.
Re-asked with that fact; owner chose our own parser for the spec's subset, with explicit
diagnostics for everything outside it and proptest round-trips against `dxf::write`.

## D15 — Kernel inputs are sanitised at the boundary (2026-10-24)

`cavalier_contours` asserts (debug) and misbehaves (release) on zero-length arc segments.
Every polyline entering a boolean, offset or stroke passes `pline::sanitize` (coincident
vertices within `REPEAT_EPS = 1e-6 mm` merged, closing duplicate removed), and arc
capsules whose inner offset arc would collapse fall back to flattened line capsules.
Found by the export pipeline test on the board fixture.

## D16 — Gap fill: Voronoi input must be provably valid (2026-10-24)

`boostvoronoi` requires segments that meet only at endpoints and panics (debug) or
returns garbage (release) otherwise; on wasm a panic aborts the worker. Sliver boundaries
are therefore snapped to the finest i32 grid (never a coarse one, which folded thin
slivers onto themselves) and checked with exact integer orientation tests before the
diagram is built; invalid slivers are skipped and reported (`pattern.gap_fill_skipped`).
Curved Voronoi edges are chord-approximated, which is exact enough for sub-pitch slivers.

## D17 — Layer operations take two operand groups (2026-10-24)

Owner feedback: subtraction as "first checked input minus the rest" depended on click
order. Every binary operation is now `union(A) op union(B)` with `inputs` (A) and
`operands` (B) as two independent checkbox groups; union simply merges both. Clipping (M2)
will be intersection with B.

Schema policy, also from the owner: no migrations before anything is deployed. The schema
stays at version 1 and is redefined in place; the migration table exists but is empty, and
a profile claiming an older version fails with a precise "no migration path" error. The
first real migration is written when a deployed version exists to migrate from.

## D18 — Strokes are built from side offsets, not capsule unions (2026-10-24)

The first `stroke` unioned one capsule per segment. Adjacent capsules share identical cap
circles and touch neighbouring arcs tangentially, and a DXF bulge rounded to six decimals
turned exact tangencies into near-tangent intersections that the n-loop boolean could not
classify consistently; the hole of a stroked outline vanished silently. Strokes now come
from cavalier offsets: a closed path is `offset(+r) − offset(−r)` of its filled loop, an
open path is its two side offsets joined by semicircular caps; capsules remain only as a
fallback when an offset fragments. The stitcher also asserts in debug builds when a chain
does not close, so an inconsistent boolean fails a test instead of returning a partial
result. Contour fill's uncovered cores are likewise derived by offsetting the region past
the last ring rather than by stroking the rings.

## D19 — Envelopes are a layer kind (2026-10-24)

The spec's five domain providers for inverted regions could have been a property of every
region layer (an implicit domain applied before subtraction) or a layer of their own. Owner
chose a separate `envelope` layer kind: it has a reference set (its inputs) and a provider
(`namedBoundary`, `boundingBox`, `boundingCircle`, `buffer`, `convexHull`), produces a
region, and is used as an ordinary operand, so an inversion stays the plain `A − B` the rest
of the model already has. Consequences: the domain is visible and previewable in the layer
list; one envelope can serve several layers (the negative preset subtracts a buffered copper
from a named board interior); reference-only envelopes are never exported. The point-based
providers (box, circle, hull) work from the reference's points, so open widthless linework
is a valid reference for them; the region-based ones (named boundary, buffer) go through the
normal region interpretation and report open linework the way a region layer does. No
reference geometry at all is `eval.envelope_unbounded`; a margin or distance that collapses
the shape is `eval.envelope_collapsed`. A subtraction whose B reaches outside A reports
`eval.subject_outside_domain` instead of clipping silently.

## D20 — End-to-end tests: Playwright on the installed Chrome (2026-10-24)

Choices: keep the DevTools-protocol scripts as a manual check; Playwright with its own
downloaded browsers; Playwright driving the Chrome already on the machine
(`channel: "chrome"`). Owner chose the last. Rationale: no browser download outside Nix, and
the thing under test is the real production bundle (`web/dist`) with shared memory, the
rayon pool, the service worker and offline mode, which only a real browser exercises.
`just e2e` is separate from `just test` because it needs `just build` and a local Chrome.
The suite runs two servers: Vite preview with COOP/COEP headers, and a header-less static
server for the service-worker path.

Found while writing the suite: the page half of coi-serviceworker reloaded on `updatefound`,
before the (precaching) worker had activated, so a fresh browser profile ended on an
uncontrolled, un-isolated page. It now reloads after `navigator.serviceWorker.ready`, at
most once per session, which also makes the first visit deterministic for users.

## D21 — Boolean engine indexes loop bounding boxes (2026-10-24)

`resolve` compared every loop pair and probed winding against every loop, quadratic in the
number of loops; a dense 5000-pad board took 3 s per boolean. A uniform grid over loop
bounding boxes (`region::BBoxGrid`) now serves pair candidates, winding probes, nesting and
island grouping. Chosen over an R-tree crate because loops on PCB artwork are similar in
size, the grid is deterministic and adds no dependency. Measured on synthetic boards: dense
5000 pads 3.0 s → 121 ms, sparse 240 → 21 ms, one large blob 530 → 92 ms. This closes the
M1 question whether `i_overlay` would be needed for performance.

## D22 — M3 readers: usvg and lopdf as libraries, Gerber (and later GDSII, IPC-2581) our own (2026-10-24)

Spike numbers on top of the 1.05 MB wasm: usvg 0.49 without text +0.52 MB / 22 crates,
lopdf 0.46 +0.75 MB / 61 crates, gerber_parser 0.5 +1.75 MB / 49 crates (regex, chrono,
strum). Owner chose libraries where they buy fidelity we would otherwise get wrong (SVG CSS,
`use`, symbols, viewBox; PDF xref/object streams and filters) and our own parsers where the
grammar is small: RS-274X including aperture macros, step-and-repeat, polarity and X2
attributes. PDF content-stream geometry (path operators, transforms, colours) is our code on
top of lopdf's object model. Owner added GDSII (binary records; own parser) and IPC-2581
(XML via roxmltree; own interpretation of the feature dictionaries) to M3 after the
spec's three formats. Order chosen: Gerber → path optimizer → SVG → PDF → GDSII →
IPC-2581 → SVG/DXF export, each committed and usable on its own.

## D23 — Source layers carry a function; slots match by file-name glob and function (2026-10-24)

One-layer-per-file formats (Gerber) and formats that declare what a layer is (Gerber X2
`TF.FileFunction`, IPC-2581 `layerFunction`) need rules beyond the exact layer name. Owner
chose: a case-insensitive glob on the file name (`*` and `?` only, no regex) and a glob on
a per-layer `function` string that each reader fills in when the format declares it
(`Copper,L1,Top`, `Profile,NP`, …). Both are ordinary `MatchRule` variants, so the
first-match-wins slot semantics and the explicit-mapping UI are unchanged. The source
document type is shared by all readers (`source::SourceDocument`); a file is detected by
extension and content, never by trusting the extension alone.

## D24 — Path order planning: per-group plans, raster footprint, squared coverage (2026-10-24)

The spec's optimizer (§7) is implemented as a plan per output group that reorders paths,
reverses open paths and rotates the seam of closed loops, never touching mark geometry.
Construction is a deterministic nearest-entry walk over a grid of entry points (both ends
of open paths, every vertex of a loop) that only offers paths whose precedence
predecessors are done; cutting passes emit "hole before its outer" constraints. Improvement
is or-opt and 2-opt with spatial neighbour candidates and delta evaluation of the affected
jumps only; a prefix reversal is allowed when no start point is assumed so the first path is
not pinned. Orientation and seam are then settled by a small dynamic programme.

The bleed penalty is geometric: each jump's footprint is a capsule of the configured width,
accumulated on a raster whose cells are flagged protected by a scanline fill of the
protected layers' regions. The objective is jump length plus `bleed_weight × Σ coverage²`
over protected cells, scaled to millimetres. Squaring is what turns "avoid hotspots" into a
number: stacking two footprints on one spot costs four, spreading them costs two. The same
raster is the preview heatmap. Nothing here is a dose and the labels say so.

## D25 — Corner joins, end spacing and ring simplification (2026-10-25)

The spec asks for a join style on boundary traces and buffers and lists "end spacing" and a
simplifying tolerance for contour fill without defining them; cavalier_contours only offsets
with round joins. Choices made without a separate question (owner may redefine):

- **Joins** (`round`, `miter { limit }`, `bevel`) on boundary traces and on the buffer and
  buffered-hull envelopes. Built from the exact round offset: each join arc (radius equal
  to the offset distance, centred on a corner of the original boundary) is replaced by the
  extended edges or by its chord. Arcs of the original geometry are never touched. A mitre
  whose tip exceeds `limit × distance` or would cross other geometry becomes a bevel; a
  bevel's chord lies within the offset distance of the corner and cannot cross anything.
  Editing the outline avoids booleans on coincident arcs, which the n-loop engine handles
  poorly.
- **End spacing** is the counterpart of the first offset: the distance beyond the last ring
  from which the centreline gap fill must still cover the core. Default `pitch / 2`, the
  previous behaviour. Larger values keep gap-fill centrelines away from the last ring.
- **Simplify** (contour fill rings): arcs whose sagitta is within half the tolerance become
  chords, then Douglas-Peucker runs over straight runs with the other half; larger arcs are
  kept exactly. Clamped to a quarter of the pitch so neighbouring rings cannot touch.

## D26 — Arcs stay arcs end to end (2026-10-25)

Owner question: "Why is everything tessellated into line segments?" The geometry model was
already lines plus exact arcs, but the preview flattened at a fixed 5 µm (visible facets
past about 200 px/mm, exactly the zoom needed to inspect a 20 µm fill), PDF and SVG
exports flattened at 1 µm, and SVG/PDF circles arrived as Béziers and were flattened at
import. Owner approved removing all four:

- **Preview** buffers carry vertices and bulges; the canvas draws arcs with `arc()` and
  falls back to a line only when the sagitta is under a quarter pixel.
- **SVG export** writes arc commands (exact).
- **PDF export** writes cubic Béziers, split so each piece is within the curve tolerance
  (default 1 µm; the former "arc flattening tolerance" setting) of the arc. PDF has no arc
  operator; this is the closest exact-enough form and far smaller than polylines.
- **SVG/PDF import** recognises a cubic as a circular arc when its end tangents are
  symmetric about the chord and it stays within `max(1 µm, 0.03 % of the radius)` of the
  circle. The relative allowance is the error the standard Bézier approximation of an arc
  itself makes, so a circle drawn as four Béziers comes back as the exact circle it stands
  for rather than as a polygon within 1 µm of the Bézier. Ellipses and general Béziers
  are still flattened.

## D27 — CI on GitHub Actions with the Nix dev shell; deploy to GitHub Pages (2026-10-25)

Owner asked for CI with GitHub Actions and publishing to GitHub Pages. CI runs the same
`nix develop -c just …` commands as a developer (lint, test, build, browser tests), so the
pinned toolchain is the one CI uses and there is no second set of setup steps to drift.
Browser tests use the runner's preinstalled Google Chrome, matching D20 (no Playwright
browser download). Only cargo/npm downloads and `target/` are cached; the Nix store is
fetched from cache.nixos.org each run until that proves slow. Pages deploys with the
official `upload-pages-artifact` / `deploy-pages` actions after a green run on `main`; the
static bundle already works from a sub-path and the service worker supplies the isolation
headers Pages cannot send (D5).

## D28 — Default path order: bleed-aware for the PCB presets, minimum jump otherwise (2026-10-25)

Owner request. The positive PCB preset exposes photoresist by laser direct imaging, where
jump footprints over areas that must stay unexposed matter, so it plans bleed-aware and
gets two reference layers: an artwork box (copper bounding box + 1 mm) and "Keep clear
(background)" = box − copper, which it protects. The negative preset ablates the
background; jumps run with the beam not Q-switched, far too weak to ablate, so ordering is
not physically critical there. It also plans bleed-aware, protecting the copper, and the
owner accepts either that or minimum jump. General marking, boundary cutting and new
profiles default to minimum jump (the model default changed from as-generated). Cost on
the 20 µm board: about 0.5 s per full evaluation in bleed-aware mode against 60–90 ms
otherwise; the plan is cached while the paths do not change.

## D29 — M4 scope: fuzzing, browsers, schema v2, packaging (2026-10-25)

Decided by the owner at the M4 kickoff, after spikes:

- **Fuzzing:** cargo-fuzz (libFuzzer, coverage-guided) with one target per reader and one
  end-to-end target, seeded from the fixtures. Every crasher is minimised into
  `fixtures/fuzz/` and replayed by `just test`; `just fuzz` runs a target locally and a
  scheduled CI workflow runs each target nightly. A 90 s throwaway mutation run over the
  fixtures had already found a Gerber panic on non-UTF-8 input, a compositing hang at
  2.5 GB on an extreme coordinate and a 5.8 M-primitive GDSII array.
- **Browsers:** Chrome and Firefox, the spec's pair. Firefox is Playwright's build from
  nixpkgs (`playwright-driver.browsers`), the same revision the npm Playwright expects; the
  installed Firefox 148 cannot be driven over WebDriver BiDi yet. Safari/WebKit is
  documented as unsupported (the app never becomes ready there).
- **Schema v2:** the coverage audit's effective marking width is saved per output group;
  this is the first schema change since deployment (2026-10-25), so it comes with the first
  real migration (v1 → v2). The deployed v1 app rejects v2 profiles with its precise
  "newer schema" message rather than dropping the field silently.
- **Packaging:** a versioned tarball of the static site built by `just package`, and a
  hermetic `nix build` package. Publishing GitHub Releases is deferred (owner, same day).
- **Bundle size:** left as is. The release wasm is 2.6 MB, 1.05 MB gzipped; `wasm-opt -Oz`
  saves nothing over `-O3` and `opt-level = "s"` saves 12 % gzipped at a speed cost. The
  service worker caches it after the first visit.

## D30 — Boolean robustness: kernel-consistent tolerances, reported incompleteness (2026-10-25)

Fuzzing found three defects in the n-loop boolean (`region::resolve`):

- A loop that crosses itself was never split at its own crossings, and a split point
  coinciding in space with another (a self-crossing) was merged with it. Bow-tie fills
  came out empty and self-intersecting regions kept a negative lobe (DEF-5). Loops are now
  split at their self-intersections, and two splits only merge when they are close in
  space *and* along the loop.
- Boundaries within the kernel's own tolerance (cavalier_contours treats points within
  10 nm as equal) were intersected inconsistently: the union of a pad flashed twice 1 nm
  apart was empty (DEF-6). Our join tolerance was 1 nm, finer than the kernel's. It is now
  the kernel's 10 nm, and vertices of different loops within 30 nm are snapped together
  before intersecting, so near-duplicates become exact duplicates. Segments shorter than
  the join tolerance are removed first. A sweep of circle and square pairs offset by
  10 pm to 10 µm in 12 directions is now clean.
- What still cannot be made consistent (near-coincident edges and arcs whose vertices do
  not line up) is no longer dropped silently: `Region::incomplete` records that a boundary
  chain did not close, booleans propagate it, and the evaluator (on the layer where it
  arises) and the Gerber, IPC-2581, PDF and SVG readers report it as a warning.

Two more guards came from a second fuzzing run: exact duplicate loops (a pad flashed
hundreds of times in place by a step-and-repeat with zero step) are merged into one loop
with a count before classification, since each slice's winding probe otherwise visited
every copy; and booleans refuse loops with non-finite coordinates or coordinates beyond
10 km, which reached the kernel's spatial index from a PDF transform and crashed it.

Tried and dropped: retrying an inconsistent boolean with coarser tolerances (up to 1 µm)
or with each loop moved by a tiny distinct offset. Once self-intersections were handled
neither rescued a single additional fuzz case.

(D34 resolves the reported cases: a suspect or unclosed boolean is checked against, or
replaced by, a robust polygon boolean.)

The kernel's debug-only self-checks fire on exactly such near-degenerate input and its
release build handles it, so they are disabled in dev and fuzz builds; our own
consistency check reports the consequences.

## D31 — Import work budget (2026-10-25)

Readers had depth limits but no bound on total expansion: DXF block arrays of block
arrays, Gerber step-and-repeat (one fuzz input asked for 15 million copies), GDSII
references, PDF forms invoking forms, and IPC-2581 dictionary primitives placed by many
pads all multiply a small file into unbounded memory or time. Every reader now charges a
shared `source::Budget`: one unit per vertex emitted (roughly 24 bytes), six per
primitive, one per instance, array cell or content operator, and 100 + 10 per vertex for
each boolean a reader performs (PDF and SVG clipping). The default limit,
`MAX_IMPORT_WORK` = 10 million units (about 240 MB of geometry, a quarter of the 1 GiB the
worker can address), is a `ReadOptions` field so tests use small ones. A file that
exhausts it is refused with one clear error rather than imported partly; GDSII's separate
5 million instance cap is gone. PDF forms that invoke themselves (a reference cycle) are
skipped with an error instead of recursing to the depth limit.

## D32 — Coverage audit per pass, width per output group; parallel booleans (2026-10-25)

Spec §10 asks for fills to be audited against an assumed swept marking width, reporting
spill, missed area and short disconnected segments. The width belongs to the output group
(one beam setting per group, D29): `auditWidthMm`, schema v2, absent = no audit. Each pass
that fills a region and belongs to an audited group gets an audit: its paths stroked at
the width (`stroke_many`), missed area = region − swept, spill = swept − region, and the
number of paths shorter than the width. It is cached on the pass fingerprint and the
width, cancellable, and shown with the pass's properties and as a red (missed) and amber
(spill) overlay. The presets audit their fills at the fill pitch, the width at which
rings exactly tile the region; on the board fixture that reports 1.19 % missed (the
corner slivers between rounded and sharp ring corners) and no spill.

The first real migration (v1 → v2) carries a v1 profile over unchanged, so a migrated
profile has the audit off; the four v1 presets, saved as the deployed app wrote them, are
fixtures of that migration (`fixtures/profiles/`).

Stroking thousands of rings made the audit as expensive as the fill, so two things that
help every boolean came with it: `Region::union_all` unions many regions in one boolean
(their loops are already oriented, so the union is where the summed winding is
positive), and `resolve` splits its input into clusters of loops whose bounding boxes
overlap, directly or through other loops. Loops in different clusters can neither cross
nor contain each other, so each cluster is an independent boolean, solved in parallel
where a thread pool exists. The audit of a 4×8 panel went from 2.9 s to 1.4–1.9 s
(measured on a heavily loaded machine; the comparison held in every run).

## D33 — After M4: Nix cache, exact booleans, parallel evaluation, graph view (2026-10-25)

Owner's follow-ups to M4:

- **CI caches the Nix store** with Magic Nix Cache (DeterminateSystems), a binary cache on
  the repository's GitHub Actions cache: free, no account or secrets, per store path.
  (Removed again by the owner once measured: no gain, see the log.)
- **Booleans must resolve near-coincident boundaries** (the case D30 only reports): snap
  vertices onto nearby edges of other loops and make nearly identical segments identical,
  so such boundaries become exact overlaps. (Done differently: D34.)
- **Layer compositing and path optimization run in parallel**: independent layers, passes
  and groups evaluate concurrently, the coverage raster and the optimizer's candidate
  search use the thread pool. Results must not depend on the number of threads.
- **A graph view of the model**, as a tab next to the preview: files and their layers →
  profile slots → working layers → passes → output groups, with live status on each node.
  It is also an editor: edges set operands, slot mappings and pass groups; layers,
  passes, groups and slots can be added and deleted on the canvas; profile edits get
  undo/redo. Drawn with Svelte Flow (@xyflow/svelte, MIT) and laid out left to right by
  dagre (@dagrejs/dagre, MIT); Svelte Flow has no layout engine of its own.
- Firefox start-up after the service-worker reload is not a focus for now.
- UI polish is postponed.

## D34 — Near-coincident booleans: exact first, checked against a robust polygon boolean (2026-10-25)

D33 asked for booleans that resolve near-coincident boundaries instead of reporting them.
The plan there, snapping vertices onto nearby edges of other loops, made things worse and
was dropped, as were larger probe distances and clustering crossing points into nodes:
each fixed some cases and broke others, because cavalier_contours still intersects the
moved boundaries within its own tolerance and may disagree with itself about where they
meet.

The exact, arc-preserving boolean stays the primary path, with three repairs:

- Vertices snap to the *nearest* vertex of another loop within the snap tolerance (they
  took the first one found, which could leave a sliver).
- Loops of the same shape within `max(extent·1e-7, 100 nm)` merge with a count, as exact
  duplicates already did (D30); this covers copies a few tens of nanometres apart.
- Chains that miss each other by a few join tolerances are bridged when stitching.

Each of these, and merging two distinct crossing points into one, marks the cluster as
suspect. A suspect cluster, or any result that is not a valid region (just inside every
boundary the winding must be 1, just outside 0), is checked against i_overlay (MIT, pure
Rust), a robust integer polygon boolean, on the same loops with arcs flattened within the
same-shape tolerance (0.1 µm up to about a metre of extent) and duplicate counts honoured.
The exact result is kept when it is a valid region with the same number of loops as the
robust one (ignoring specks a few tolerances wide) and an area within four flattening
tolerances times the input perimeter; otherwise the
robust result replaces it, in that cluster only, without arcs. A cluster whose exact
boolean cannot close at all also takes the robust result. `Region::incomplete` and the
`*.incomplete_boolean` warnings remain for a loop that cannot be flattened.

Evidence: a sweep of squares, circles, capsules and irregular polygons combined with a
copy moved, rotated or scaled by 10⁻¹¹ to 10⁻³ mm (union, intersection, difference, xor,
union of three, a hole) runs 200 cases in `just test` and 40 000 cases (240 000 booleans:
6.5 s in a release build, three minutes in `just regression`'s dev build on a loaded
machine) in `just regression`, with no incomplete and no wrong result. All 464
saved fuzz inputs now evaluate without an incomplete composite. On the board fixtures the
robust result never replaced an exact one, and the panel benchmark did not slow down.

Cost: i_overlay and its four helper crates add 266 kB to the wasm module (97 kB gzipped,
about 9%).

## D35 — Parallel evaluation that cannot change results (2026-10-25)

D33 asked for parallel composition and optimisation whose results do not depend on the
number of threads.

- **Layers** evaluate in dependency waves (one more than the deepest input's wave); the
  layers of a wave are composited concurrently. A task graph would start a layer as soon as
  its own inputs are done, but the heavy work sits inside single layers (unions, offsets,
  fills), which are parallel themselves.
- **Composition**: a region layer's parts (stroked paths, hatches, filled loops) are built in
  parallel and united in one n-ary union. Folding pairwise unions re-resolved the growing
  region for every part and was the whole cost of compositing (0.92 s → 0.01 s for the
  8×8 panel, natively). The composite is the same region; its loops may start at other
  vertices, which shifts contour-fill seams slightly.
- **Passes** (pattern and coverage audit) and **output groups** (plans) run concurrently.
  Whatever completes is cached, also when the evaluation is cancelled meanwhile.
- **Optimiser**: the plans are bit-identical to the sequential ones. The local search still
  applies, per sweep, the first move in scan order that pays off; positions are scanned in
  parallel blocks against the unchanged sequence and the candidates offered in scan order.
  Precedence is checked by computing positions after the move instead of building the new
  order. Footprint cells are visited row by row over an interval containing the capsule,
  with the same exact distance test, so probes are read-only and a full re-stamp runs in
  parallel bands of rows (counts are integers). The seam programme's jump costs and the
  crossing count are parallel too. Lifting the one-move-per-sweep limit was measured and
  rejected: 2 000 sweeps shorten the 4×4 panel's jumps by 0.4%.
- `tests/parallel_determinism.rs` compares whole evaluations (geometry, paths, audits,
  plans, diagnostics) with parallelism off and on 1, 3 and 8 threads, on a panel in
  bleed-aware mode and on a cutting grid with precedence constraints.

Natively, on a machine shared with a load average between 20 and 100, the 8×8 panel went
from 8.3 to 7.4 s on one thread and from 2.7 to 1.8 s on ten (bleed-aware). The coverage
audit is now most of the work; computing both of its differences from one intersection
pass would about halve it.

## D36 — The wasm build's allocator: per-thread caches over a sleeping heap lock (2026-10-25)

Timing D35 in Chrome showed the 8×8 panel taking 53 s per evaluation with ten threads,
17 s with four, 10 s with two and 16 s with one. std's allocator for wasm guards one
dlmalloc heap with a spin lock: every allocation of every thread queues on it, and with
more threads than free cores the spinning starves the thread that holds it. std cannot
block there because a browser's main thread may not wait; our module only runs in workers.

`lvp-wasm` therefore installs its own global allocator: small blocks (up to 2 KiB, in 28
size classes) are cached in 32 slots and move to and from the heap in batches, and the
heap (the same dlmalloc) sits behind a lock that spins briefly and then sleeps with
`memory.atomic.wait32` (sleeping at once made cancellation sluggish in Firefox under load:
every hand-over of the lock waited for the scheduler). A
thread picks its slot by its stack address, because wasm-bindgen allocates a new thread's
stack and TLS block through this allocator before that thread's TLS exists; a slot is only
tried, and a busy one sends the call to the heap. The cache logic is generic over its heap
and tested natively by threads that allocate, resize and free blocks of every size, many
freed by another thread. With it the panel takes 3.3 s per evaluation in Chrome with ten
threads (13.6 s with one), and 2.7 s for import and first evaluation instead of 72 s; in
Firefox 8 s instead of 37 to 49 s, measured while the machine was far busier. Cost: the
module grows by 82 kB (21 kB gzipped); std's own copy of dlmalloc stays linked.

## D37 — More patterns: ring order, linking, centreline, dots, breaks (2026-10-26)

The owner asked for more of spec §6's additional patterns, starting with outward contour
fill, and chose centreline engraving, linked spiral and hatch, dot lattices, and dashes and
tabs. Outward contour fill means marking the rings from the core out: growing rings outward
from a layer and clipping them to a domain can be built from layer operations, so it is no
pattern.

- **Ring order is a constraint, not a generated order.** The optimizer reorders paths, so a
  fill generated core first would not stay that way. Each ring, bridge and gap-fill
  centreline is tied to the nearest path of the ring level just outside it (a forest, one
  pair per path), inward or outward. Checking every pair for every candidate move would not
  scale to the 23,000 pairs of the 8×8 panel, so the optimizer now checks a relocation
  against the moved path's own pairs and a reversal against pairs inside the reversed run;
  a proptest compares this with checking every pair. Outward order costs the 8×8 panel
  4.4 s instead of 3.6 s natively, and longer travel.
- **Linked rings.** Working inside out, each ring's seam is where the ring inside it
  connects, and its connector runs from there to the nearest point of the ring outside.
  That point is exactly a pitch away (rings lie at constant depth), so the connector
  stays in the band between the two rings and crosses nothing; a ring already taken by
  another chain starts a new one. Connectors are marks (drawn and audited). A chain is an
  open path whose direction is the order, so passes now also report **directed** paths:
  the optimizer offers them only their start, keeps their orientation in polishing and
  relocation, and does not reverse a run that contains one.
- **Gap fill at every level** (DEF-15): the cores a ring level leaves are the points deeper
  than the ring plus the end spacing that are not within `pitch − end spacing` of the next
  level. A round sweep misses a sliver at every convex corner; corners of 60° or wider
  count as swept (as if mitred; the audit still reports them), sharper ones are cores. A
  level whose next level has the same numbers of loops and vertices and no sharper corner
  cannot hold a core (nothing collapsed between them) and is skipped. On the 8×8 panel the
  fill takes 0.06 s instead of 0.014 s at 10 threads; in the browser the whole evaluation
  grows by about 0.2–0.4 s of 3–4 s.
- **Centreline** is the gap fill's medial axis of the whole region. Curved stretches of the
  axis (between a corner and an edge) are followed as parabolas, within 1 µm.
- **Linked hatch** joins a row end to the next boundary point along its loop when that is
  the start of a segment in the next row and the stretch is at most `link_mm`: the link
  runs on the boundary, so it never crosses a hole or passes another row end.
- **Dots** come from the hatch's row spans, so they share its global lattice. A dot is a
  small circle or a short dash, never a zero-length point. The stipple is the hex lattice
  with each dot moved by up to 0.3 pitch by a hash of its lattice point: irregular, the
  same on every run and in every region.
- **Breaks** apply to any pass after its pattern. A constraint between two paths becomes
  one from the last piece of the first to the first piece of the second, with each
  constrained path's pieces kept in order; pieces of a directed path stay directed.
- **Schema v3** with an identity migration: the deployed v2 app would otherwise read a v3
  profile and silently drop the fields it does not know (dashes on a cut, for one).
