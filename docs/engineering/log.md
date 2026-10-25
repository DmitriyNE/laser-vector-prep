# Engineering log

Newest entry first. Record what was done, what broke, and what was learned; decisions
with lasting effect go to `decisions.md`.

## 2026-10-25 — After M4: graph view

The main area has a Graph tab next to the preview (D33): imported files with a row per
layer, slots, working layers with their operand groups A and B, passes and output groups,
connected as the profile and the bindings connect them, each with its status from the
latest evaluation. A click selects into the properties panel and the eyes act on the
preview as in the layer list. Svelte Flow 1.7 draws it; dagre 3.1.1 (pinned to a release
some months old) orders each column. Letting dagre place everything gave a diagonal
staircase, because a virtual root pinning the columns dragged long chains of dummy nodes
through the layout; the columns are now assigned directly and dagre only orders them.
The graph code is a separate chunk loaded on first use (225 kB, 73 kB gzipped), so the
page itself grew by 11 kB; the service worker precaches it with the rest for offline use.
Svelte Flow styles its own node types (`input`, `output`, `default`, `group`), so output
groups are nodes of type `outputGroup`.

The graph is also an editor. Drawing from an imported layer's row to a slot maps it, from a
slot or layer into input A or B of a layer adds an operand (refused, with the reason, if it
would make a cycle or is there already), from a layer to a pass moves the pass, from a pass
to a group sets its group; Delete removes operand edges, layers, passes, files, and slots
and groups nobody uses. A deleted layer now also leaves the inputs of the layers that read
it and the optimizer's protected list (the layer list used to leave them dangling), and the
groups panel refuses to delete a group a pass marks in. Every profile edit is undoable
(Ctrl/⌘ Z, Ctrl/⌘ ⇧ Z, buttons): the store keeps a snapshot after each edit, and typing on
in one field extends the step. The first version merged any edits less than half a second
apart, which the browser test caught by undoing four quick graph edits in one step.

First use by the owner: connecting by dragging was not discoverable (the handles were 3 px
at the fitted zoom) and no edge could visibly be removed (only a selected operand edge
with Delete). Handles are now larger with a ring while hovered, connections snap within
36 px, the toolbar says how to connect and remove, and removable edges carry a × in their
middle: an operand input, and a mapping, whose × removes every rule of the slot that
matches the layer (patterns too, which needs the core's matching in the page; the toolbar
names the rules removed). Clicking any other edge says how to change it. A redraw after
an edit or evaluation used to re-select the store's selection on the canvas, so Delete on
a freshly selected edge also deleted the layer selected before; the canvas now keeps its
own selection until the selection changes elsewhere.

The preview has a minimap like the graph's, for a consistent main area: same size, corner
and colours as Svelte Flow's (light and dark), the drawing with everything outside the view
masked, drag to pan (pressing outside the view jumps there) and wheel to zoom. The drawing
is rendered into it once per fetch; panning only redraws the mask, since redrawing a large
panel on every move would not keep up.

## 2026-10-25 — After M4: parallel evaluation, a wasm allocator

Layers, passes, audits and plans now run concurrently and the optimiser's scans use the
pool (D35), with results identical for any thread count. Before changing the optimiser,
every plan of the four presets in both modes on 2×2 and 4×4 panels, and of a cutting grid
with 800 precedence constraints, was hashed; the new code reproduces all of them on one
and on ten threads. Compositing turned out to be quadratic: each part of a region layer
was united with everything before it.

Timing the result in Chrome showed the real problem: ten threads took 53 s for an 8×8
panel, two threads 10 s. std's wasm allocator spins on one lock (DEF-13). The wasm build
now caches small blocks per thread in front of a heap whose lock sleeps (D36), and the
panel takes 3.3 s. The browser figure in the M4 notes (2.6 s) predates the coverage
audit, whose booleans allocate heavily; nobody had timed the panel in a browser since.

Timings this session are rough: another project kept the machine at load averages of 20
to 260, so old and new builds were always run alternately, side by side.

The first CI run after that failed one browser test: Firefox took 5.02 s to cancel a 1 µm
fill (the limit is 5 s). With the faster allocator the fill now finishes early and the
cancel lands in its coverage audit, whose stroking and clustering never checked for
cancellation (DEF-14); a native reproduction took 3.3 s to stop, a debug build 18 s.
Sampling the stuck process (`sample <pid>` on macOS) pointed straight at the clustering.

## 2026-10-25 — After M4: near-coincident booleans, Nix cache measured

Booleans no longer leave near-coincident boundaries unresolved (D34): the exact boolean
runs first, and a cluster it had to nudge, or whose result is not a valid region, is
checked against i_overlay's robust polygon boolean, which replaces it when they disagree.
A randomized sweep (squares, circles, capsules, irregular polygons against a copy shifted,
rotated or scaled by 10⁻¹¹ to 10⁻³ mm) is in `just test` at 200 cases and in
`just regression` at 40 000; all 464 saved fuzz inputs evaluate without an incomplete
composite.

What did not work, in order: snapping vertices onto nearby edges of other loops (D33's
plan), larger winding-probe distances, and clustering crossing points into shared nodes.
Each repaired some sweep cases and broke others. The sweep also showed that vertex snapping
took the first vertex in range rather than the nearest, which left slivers (DEF-12).
Reviewing the fallback before committing found that it collapsed a merged duplicate's count
to one copy for the non-zero rules: a square twice plus the same square with a hole lost
the hole's area in a union. It now passes every copy, with a test.

Magic Nix Cache (D33) works but did not speed CI up. The dev-shell step takes 35 s with it,
as before, and a second `nix build` of the same commit took 2:14 against 1:55 for the
first. Uploads stopped after exactly 200 cache entries in the first minute of each run,
which matches a per-repository rate limit, so the second run rebuilt 172 derivations.
Most of the closure comes from cache.nixos.org anyway; what costs time is the cargo build,
which CI already caches in `target/`, and the site derivation, which changes with every
commit.

## 2026-10-25 — M4: packaging and deployment

`just package` writes a reproducible tarball of the site (two builds of one commit gave the
same SHA-256), and `nix build` builds the site hermetically: cargo sources vendored from
`Cargo.lock` plus the standard library's lockfile for build-std, npm sources pinned by
hash, and the same `just build` inside. The vendoring that D29 expected to be fiddly worked
on the first try; the Nix-built site passed the full live check (isolation through the
service worker, import, PDF export, offline reload, no console errors). GitHub Releases
stay deferred. `docs/deployment.md` covers hosting with or without isolation headers,
sub-paths, caching and offline use.

The tarball recipe needed git inside the dev shell: on macOS `/usr/bin/git` is Apple's shim,
which fails under the Nix developer environment ("tool 'git' not found").

## 2026-10-25 — M4: preview precision

Preview buffers held absolute `f32` millimetres, which step by 8 µm at 100 m from the
origin and 60 µm at the 1 km import limit, coarser than a 20 µm fill. Each buffer now
carries its lower-left corner as `f64` and `f32` offsets from it; the canvas adds the two
in double precision.

## 2026-10-25 — M4: profile JSON Schema

`docs/profile-schema.json` is generated from the serde types with schemars (doc comments
become descriptions) and checked by `just test`: it must equal the generated one, accept
every preset and the saved v1 profiles, and reject a few malformed documents
(jsonschema, test-only). `just schema` regenerates it. The doc comments that said
"`None`" now say "absent", which reads right in the schema too.

## 2026-10-25 — M4: coverage audit and schema v2

Fills are audited against a per-group assumed marking width (D32): missed area, spill and
short segments with each pass, and a missed/spill overlay in the preview. The profile
schema is now version 2, with the first migration and the v1 presets as its fixtures.

The audit test first expected only the outer ring to leave corner gaps; every ring does,
because a ring's stroke has rounded outer corners while the ring outside it has sharp
inner ones, (1 − π/4)·r² per corner per ring. Making the audit affordable on panels
produced `Region::union_all` and parallel cluster-wise booleans. This machine was running
another project's test suite at a load average of about 100 meanwhile, so the timings in
D32 compare runs made side by side rather than absolute numbers.

## 2026-10-25 — M4: Firefox

The browser suite runs in Chrome and Firefox (D29). Firefox is Playwright's own build from
nixpkgs, trimmed to Firefox alone (294 MB instead of 2.4 GB with Chromium and WebKit);
`just check-playwright` keeps the npm and Nix Playwright releases equal. The installed
Firefox 148 could not be used: its WebDriver BiDi lacks a command Playwright 1.64 sends
for every new context.

Every spec on the Vite preview server failed in Firefox with "navigation interrupted by
another navigation", which looked like the service worker reloading a page that already
had its isolation headers. It was the port: 4190 (ManageSieve) is on the Fetch standard's
list of bad ports, Firefox replaces the navigation with an error page, and the servers
now use 4180 and 4181. With that, all 18 specs pass in both browsers; Firefox evaluates
the board about twice as slowly as Chrome.

## 2026-10-25 — M4: cancellation and progress

The worker cannot read messages while a wasm call runs, but the page can write into the
module's shared memory: `lvp_core::cancel` keeps a small control block there, and every
import or evaluation runs under an epoch the page can cancel, queued or running. The
store now keeps at most one evaluation in flight; an edit cancels it and re-evaluates
instead of queueing behind it. A progress bar and a Cancel button appear after 300 ms.

Writing the tests found two gaps: a cancel during the copper compositing of a 2×2 panel
took 17 s to take effect in a debug build until the boolean itself checked for it, and
summaries and exports read the *current* profile, so after a cancelled run they would
have mixed the new profile's group colours with the old paths. The project now keeps the
profile each evaluation was made from.

## 2026-10-25 — M4: large inputs

Contour fill offset the whole layer at every ring depth, so every island was processed
until the deepest one collapsed and the kernel related all loops to each other. Islands
are now filled one by one, in parallel through `par::map`, which uses rayon natively and
in the browser once the worker has started the pool (`enable_parallelism`); the Node-side
wasm tests stay sequential. Output is identical either way (tested), only the
as-generated path order is now island by island. Measured on panels of the Gerber board
at 20 µm (native, 10 threads): 4×8 boards 1.1 → 0.5 s, 8×8 boards 0.8 s; in Chrome the
8×8 panel evaluates in 2.6 s with minimum jump and 4.6 s bleed-aware. The kickoff figure
of 4–6 s for the 4×8 panel was measured while a wasm build ran alongside.

## 2026-10-25 — M4: fuzzing

cargo-fuzz targets for the six readers and the import → evaluate → export pipeline, with
saved crashers replayed by `just test` and a nightly workflow (D29). Two 10-minute runs of
every target found eleven defects (DEF-5 to DEF-11), most of them in the boolean rather
than the readers: self-intersecting loops were never split at their own crossings, the
union of near-duplicate pads could be empty, stacked duplicates made compositing
quadratic, and non-finite coordinates crashed the kernel's index (D30). The rest were
unbounded expansion (D31), a Gerber panic on multibyte input and a hang on extreme
coordinates. The first run produced 189 crashing inputs and 18 timeouts or out-of-memory
cases; all of them now finish, and the second run found three new inputs, now fixed.

Learned:

- `cargo fuzz` turns on debug assertions for every crate unless given `-O`; the per-crate
  profile in `fuzz/Cargo.toml` only takes effect with `-O`.
- The kernel's own position tolerance (10 nm) is the floor for every tolerance around it.
  A finer join tolerance looked more exact and was the cause of DEF-6.
- Escalating tolerances and perturbing inputs to rescue inconsistent booleans looked
  promising and rescued nothing once self-intersections were handled; measuring each
  rescue against the saved crashers kept the code simpler.

## 2026-10-25 — Path-order defaults

Bleed-aware by default for the two LDI presets, minimum jump for the rest and for new
profiles (D28). On the board fixture the positive preset's travel drops from 2647 mm to
49 mm and the keep-clear area under jump footprints from 34.3 to 2.5 mm².

## 2026-10-25 — CI and GitHub Pages

`.github/workflows/ci.yml`: lint, test, build and browser tests in the Nix dev shell on
ubuntu-24.04, then deployment of `web/dist` to GitHub Pages from `main` (D27). Writing it
showed that `just test` never installed npm dependencies; it only worked locally because
`node_modules` existed. The recipe now depends on `deps`.

The first CI run passed in under five minutes and published the site. Checking the live
site in headless Chrome found DEF-3 (worker started before isolation); the offline browser
test now fails on any console error. Rebuilding locally then exposed DEF-4: `just test`
had left the debug wasm in place, and debug wasm traps inside `hypot` during bleed-aware
planning because build-std applies our overflow checks to the standard library's libm.
The next CI run failed on a vitest timeout: the board test now plans bleed-aware by
default and took 5.4 s in single-threaded debug wasm on the runner (well under 5 s
locally). The wasm suite's timeout is now 60 s; timings measured locally do not carry over.

## 2026-10-25 — Arcs end to end

Preview, PDF/SVG export and SVG/PDF import no longer tessellate circular arcs (D26). The
board's copper rings now render smooth at 3000 px/mm, PDF and SVG exports carry curves,
and a PDF or SVG exported by the app reads back with its arcs. Readers also close a
stroked path that returns to its start, as the Gerber reader already did.

## 2026-10-25 — M3 step 8: gap fill refinements and format reference

Join styles for boundary traces and buffer/hull envelopes, end spacing and ring
simplification for contour fill (D25), with editors in the Properties panel. The mitre
crossing check first shortened the new edges by 1e-7 of their length to avoid touching the
neighbouring edges, but the kernel's fuzzy intersection test still reported a touch and
every mitre fell back to a bevel; intersections at the shared end points are now filtered
instead. Simplification first left most vertices because offsetting flattened input
creates many tiny arcs; arcs flatter than half the tolerance now count as chords.
`docs/formats.md` documents the supported subset of every reader and exporter, which is the
"documented vector PDF subset" the spec asks for, extended to all formats.

## 2026-10-25 — M3 step 7: SVG and DXF export

`export::svg` writes one Inkscape layer per exporting group on the PDF's page (y flipped,
round caps and joins, 1:1 mm viewBox); `export::dxf` writes one layer per group as
LWPOLYLINEs with bulges in model coordinates, colours as the nearest ACI, names made
DXF-safe. Both round-trip through the app's own readers in tests and in a browser spec
(export, re-import, one layer per group). This closes the M3 list from the spec plus the
owner's GDSII and IPC-2581 additions.

## 2026-10-25 — M3 step 6: IPC-2581

Own reader on roxmltree (D22): dictionaries (standard primitives Circle, RectCenter,
RectCorner, RectRound, RectCham, Oval, Donut, Octagon, Hexagon, Diamond, Triangle, Ellipse,
Thermal, Contour; user primitives; line descriptors), steps with profile and layer features,
pads with Xform/Location, positive and negative sets composited in source order, and
`layerFunction,side` as the layer function so the PCB presets bind `CONDUCTOR,TOP` and
`PROFILE` without naming conventions. The stroke-polygon helper moved from the GDSII reader
into `pline` and a centre-defined arc helper joined it; both readers share them.

## 2026-10-24 — M3 step 5: GDSII

Own stream reader (D22): records → structures → flattened top cells with the GDS reference
transform (reflect about x, magnify, rotate, translate) composed through SREF/AREF nesting
with depth and instance caps. Paths with flush, square or custom ends become exact mitred
polygons; round-ended paths stay centrelines with width. Layers are `layer/datatype`,
prefixed by the structure name when several top cells exist. A generated fixture
(`gds/cells.gds`) pins areas, counts and bounds; the reader's own 8-byte real decoding is
round-trip tested against the emitter.

## 2026-10-24 — M3 step 4: vector PDF

lopdf for the object model, our interpreter for the content stream (D22). Layers are
optional-content group names or the paint colour as `#rrggbb`, so a PDF exported by this
app comes back as one layer per output group; a round-trip test checks that on the board
fixture (same path count, 0.0254 mm widths, 1:1 extents). A lopdf-built fixture covers
forms, Béziers, even-odd, clipping, OCGs, text and images. `Unit::Point` was added so the
UI does not show PDF units as assumed.

## 2026-10-24 — M3 step 3: SVG

usvg (text feature off) resolves CSS, `use`, the viewBox and shapes; our `svg` module
maps the tree onto source layers (D22). A raw-XML pre-scan supplies Inkscape layer labels
and the diagnostics for text, images, masks, filters and foreign objects that usvg would
otherwise drop quietly. Generated fixture `svg/marking.svg` binds the general-marking
preset, whose layer-name rules are now format-agnostic.

Learned:

- usvg wraps the document in anonymous groups for the viewBox transform; "top-level
  groups are layers" must descend through id-less groups first.
- `line` and two-point paths carry SVG's default black fill; a fill with no enclosed area
  must not become an (empty) filled primitive.
- usvg geometry is f32: areas and widths reproduce to about 1e-5 relative, so fixture
  tolerances are looser than for DXF and Gerber.

## 2026-10-24 — M3 step 2: path order planning

Per-group plans (D24) with the three spec modes, precedence from cutting passes (holes before
their outer, emitted by the boundary trace), orientation and seam choice, a raster footprint
model for the bleed penalty and the heatmap, and before/after statistics in a new "Path
order" panel. Export follows the plan; jump and heatmap overlays are per group. Groups can
override the mode.

Learned:

- The first draft re-polished and re-evaluated the whole sequence for every candidate move
  and probed capsules with a 7 × 7 block per half-cell step: 90 s for 250 hatch rows.
  Delta evaluation of the affected jumps, a precomputed disk stencil with an exact distance
  filter, probes shared per node, and pruning candidates whose extra travel exceeds the
  best possible bleed gain brought the same test to 0.5 s.
- A neighbour search that scanned full squares per ring was O(ring³) on a grid of
  1 × 2000 cells; building the grid from the larger extent and walking ring boundaries fixed
  both the shape and the scan.
- Length-only polishing after a bleed-aware search undoes its decisions; the DP now takes
  the footprint probe into its transition costs.
- With no assumed start point the first path must be movable: 2-opt may reverse a prefix.

## 2026-10-24 — M3 step 1: Gerber

Shared `source::SourceDocument` for every reader, `read_any` with content-first format
detection, and two new slot rules: file-name glob and layer function (D23). The Gerber
reader (D22) is our own: lexer for `*` words and `%…%` blocks, FS/MO, standard apertures,
macros with a small expression parser, draws (chained into polylines, arcs in both quadrant
modes, a chain returning to its start becomes a closed contour), regions, SR, AB, LM/LR/LS,
polarity. Dark-only files pass flashes and draws through as primitives; files with clear
polarity are composited in source order into one region. Three generated Gerber fixtures
(copper and profile of the board, plus a features file) are read back against the manifest,
and the PCB presets bind the X2 pair through `Copper,L1,Top` and `Profile*`.

Learned:

- Re-reading a resolved region's loops with the nonzero rule loses holes: `from_loops`
  normalises every loop to CCW first. Resolved loops must be re-read with even-odd, which
  is exact for them; fills now carry outers and holes separately for that reason.
- A Gerber profile is a thin drawn contour, not a filled area. The presets that need the
  board interior (named boundary, cutting) read closed contours as loops regardless of the
  drawn width (`strokeWidth: none`), which also keeps them exact for DXF outlines.
- The macro comment primitive is `0 text` with no comma after the code; splitting on commas
  first mis-parsed it.

## 2026-10-24 — Layers panel compacted

Owner feedback after M2: the Layers panel overflowed its 300 px column and every row had
two identical checkboxes (preview visibility and enabled). Rows are now a grid: eye toggle,
two-line name (title plus a muted detail line with kind, operation, flags and "used by",
ellipsised), stats right-aligned, tools only on hover or selection. The enabled switch moved
to the Properties panel; disabled rows are dimmed and say so. Same shape for passes, with
paths over length and the delete tool in its own column. Height for the negative preset
went from about 700 px to about 400 px.

Second round, same day: source layers and files got the same eye (a shared `Eye.svelte`);
toggling a layer's eye takes its passes along; a file's eye takes all its layers. "Show
only this" via alt-click on any eye or a hover "only" button hides everything else in the
preview (a layer keeps its passes, a file its layers); alt-clicking the only visible item
restores everything, as does "show all" in the preview toolbar and panel headers. Preview
visibility stays UI state and is never written to the profile. Covered by
`e2e/visibility.spec.ts`.

## 2026-10-24 — M2: geometry and layers

Commit per sub-step:

1. `BBoxGrid` spatial index in the boolean engine (D21): dense boards from seconds to
   about a hundred milliseconds.
2. Envelope layers with the five domain providers and their diagnostics (D19); one
   evaluator test inverts the same subject against all five.
3. Pattern family: boundary trace (inside / on / outside, kerf, rings, holes first), parallel
   hatch and crosshatch with exact line/arc crossings on a global phase lattice, and
   boundary + hatch. Proptests check that every emitted segment stays inside the region
   and that nothing bridges a hole.
4. UI editors for envelope layers and the new patterns; the layer list labels the operation
   ("A − B") and the pattern.
5. Playwright end-to-end suite on the installed Chrome (D20): import → evaluate → export
   PDF, slot mapping, two-group subtraction with hatch, service worker + offline.
6. Built-in presets for the spec's four scenarios (positive LDI, negative background hatch,
   general marking, boundary cutting), each evaluated against a fixture in
   `crates/lvp-core/tests/presets.rs`.

Learned:

- A scanline exactly through a polygon vertex yields two crossings or none depending on
  rounding; the hatch nudges such lines by a sub-epsilon instead of special-casing vertices.
- Fixture bulges are written with six decimals, so areas and perimeters derived from arcs
  reproduce to about 1e-6 mm, not 1e-9. Fixture-level tests use 1e-4.
- Under Playwright the coi-serviceworker never isolated the page: the reload on
  `updatefound` raced the precache and landed on an uncontrolled page. Waiting for
  `navigator.serviceWorker.ready` fixed it (D20).
- Playwright's `channel: "chrome"` needs no download and runs headless with service workers
  and `SharedArrayBuffer`; `context.setOffline(true)` plus a reload proves the offline path.
- The first thing the presets e2e test found was DEF-1: switching presets from the bar had
  silently never worked, because `structuredClone` cannot clone a Svelte `$state` proxy and
  the `change` handler threw. Anything that copies reactive state goes through
  `$state.snapshot` first.

## 2026-10-24 — Stroke construction and silent boolean failures

While verifying the two-group subtraction in the browser, "copper minus a band around the
outline" came back empty. Root cause was not the subtraction but the stroke of the closed
outline (D18): capsule unions with coincident caps and near-tangent arcs made the boolean
drop a chain silently, and in release the gap fill produced garbage through the same path.
Fixed by offset-based strokes, by snapping intersection points onto their segments before
slicing, by deriving uncovered cores from an offset instead of stroking rings (also much
faster: the core suite went from 4.5 s to 0.5 s), and by a debug assertion on dangling
chains so the suite now fails loudly on inconsistent booleans.

## 2026-10-24 — Two operand groups

Owner feedback: subtraction depended on checkbox click order. Model changed to
`union(A) op union(B)` with two groups (D17); the properties panel shows A and B as
separate groups and the layers list labels the op as "A − B" etc. Stored profiles are
pushed through the worker on load so they are validated the same way as imports. A v1→v2
migration was written first and then removed on the owner's instruction: nothing is
deployed, so the schema is redefined in place and stays at version 1.

## 2026-10-24 — Bindings made visible

Owner feedback after the M1 demo: managing layer bindings was unclear. Their model:
bindings belong to the profile (the slots a profile needs), and imported file layers are
mapped onto them. The UI now follows that: a "Profile sources" panel lists each slot with
its rules, status (matched / missing / ambiguous) and a "map imported layer" control;
each imported layer in the Sources panel has a "→ slot" control (or "+ new slot"); the
Properties panel edits a slot's name, required flag and ordered rules. Explicit mappings
are stored as rules: a portable layer-name rule, or a file+layer rule when the same layer
name exists in several files. Verified in headless Chrome with two fixtures.

## 2026-10-24 — M1 implementation (vertical slice)

Commit per sub-step, in dependency order:

1. Region kernel over cavalier_contours: n-loop boolean (split at intersections, classify
   slices by both operands' winding numbers, stitch), fill rules incl. merge-then-nest,
   exact-arc offsets via `Shape`, stroke expansion by capsule union. Inclusion–exclusion
   proptests over random rectangles and circles.
2. Own DXF reader: group-code tokenizer, HEADER/TABLES/BLOCKS/ENTITIES, INSERT expansion
   with similarity transforms (arcs survive; non-uniform scale flattens with a warning),
   OCS mirroring, HATCH (polyline and edge boundaries, nesting styles), ELLIPSE/SPLINE
   flattening (de Boor), SOLID/TRACE as filled quads, unsupported-entity summaries with
   ACIS as errors. Fixture-driven integration tests read `fixtures/manifest.json`.
3. Model + profiles: serde schema v1, migration table, validation, PCB preset. Evaluator:
   binding resolution, Kahn ordering with cycle rejection, source interpretation, boolean
   composition, pattern passes, fingerprint cache (changing a pass recomputes only that
   pass; replacing a file recomputes dependants; reordering layers recomputes nothing).
4. PDF exporter: deterministic, strokes only, 1:1, per-group files share one layout.
5. Gap fill: missed-area audit (region minus rings stroked at the pitch) and Voronoi
   medial axis of the slivers (D16). The getrandom wasm backend came along for the ride.
6. Project facade + wasm API + Svelte UI with IndexedDB profiles.

Learned:

- cavalier_contours debug-asserts on coincident vertices with a bulge; sanitising at
  the kernel boundary (D15) fixed a crash that only the board fixture triggered.
- A coarse Voronoi snapping grid folds a thin sliver onto itself; use the finest grid and
  validate segments exactly (D16).
- `rustc --print link-args` was the quickest way to see which linker flags the toolchain
  still adds for atomics (none of the shared-memory ones).
- Several commits slipped through with failing tests because `cargo … | grep` reports
  grep's exit status; verification now writes to a log and checks cargo's status.
- The first in-browser run crashed every call after the first preview with "memory
  access out of bounds". Node could not reproduce it because the tests call the wasm
  directly. Cause: the worker request envelope did `{ ...payload, id }`, so the numeric
  request id overwrote the `id` *string* of preview payloads and the glue marshalled a
  number as a string (`realloc` with size 0, a wasm-bindgen debug assertion). TypeScript
  had flagged the clash and an `as Request` cast hid it. Lessons: never cast away a
  protocol type error; end-to-end checks must go through the real worker protocol;
  a wasm trap poisons the instance (allocator lock, `RefCell` borrow), so the client now
  replaces the worker and re-imports sources automatically.
- Verified in headless Chrome via the DevTools protocol: fixture import through the file
  input shows 10 islands / 1 hole, two matched bindings, export enabled, no console
  errors; on a plain static server the service worker installs on the first visit and
  the app loads and runs fully offline after the server is stopped.

## 2026-10-24 — M1 kickoff analysis

- Kernel spike (scratch crate, release build): `i_overlay` and `cavalier_contours` both
  compile and run under the atomics toolchain; numbers in D7. Owner chose
  `cavalier_contours` for arc fidelity; model ownership in Rust (D12); M1 runs end to end.
- `dxf` crate: compiles for wasm32 only with `uuid/js`; no HATCH entity (commented out of
  its spec). Re-asked the owner (D14).
- Fill plan written up per mode (D13): kernel primitives below, our strategies above.

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
