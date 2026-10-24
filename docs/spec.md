# Laser Vector Prep — browser-only geometry and toolpath preprocessor

**Coding-agent handoff specification · v1.3 · 2026-10-24**  
**Status:** requirements, not an implementation  
**Delivery:** static web application. All file parsing, geometry operations, path generation, preview, optimization, profile management and export must run in the browser.

## 1. Purpose and boundaries

A general-purpose **vector artwork and trajectory preprocessor for galvo laser software** with inadequate native preparation facilities. It is **not specific to PCB LDI**. Anticipated uses include photoresist exposure, vector marking/engraving, filled engraving, scoring, cutting and future operations supported through geometrical path generation.

Workflow:

`Open webpage → import drawing(s) → map/configure layers → apply geometry operations (including negative regions) → choose per-layer path patterns → preview/optimize → export color-separated vector PDF`

**Hard architectural and scope constraints**

1. **Frontend-only.** TypeScript/JS/WASM, served as static assets. No conversion service, upload endpoint, account, server job state, or database. The drawing stays in the browser.
2. **PDF exports geometry, not machine instructions.** Power, scan velocity, repetition frequency, delays, number of physical repeats, focus, duty cycle and material/process recipes are defined in Full Spectrum or another laser vendor's software. The app never claims to transmit these through PDF. PDF colors merely allow the user to assign machine processes downstream.
3. **Layer profiles ARE required.** Named geometry-configuration profiles must be stored locally and be importable/exportable as versioned JSON. They are not laser/machine/material profiles; never embed source drawing bytes by default.
4. **Mark paths and non-marking jumps are distinct.** The converter exports only intended beam-on geometry. Hypothesized beam-off travel may be simulated and optimized, but cannot be forced by an ordinary vector PDF. Path order preservation by vendor software is unverified.
5. All dimensions, geometry tolerances, pitch, margins, offsets and stroke widths are in **physical millimetres** internally. Never silently rescale, mirror or fit artwork to the output page.

**Compatibility baseline:** A Full Spectrum UV galvo/JPT SEAL-355-10 successfully exposed generic dry-film photoresist using the existing contour-parallel PDFs. The validated PDF style uses **nonzero 0.0254 mm RGB strokes**, blue for marking and red for board outline, at 1:1 scale. The machine can leave faint marks during supposedly beam-off jumps; optimizing concentrated overlap is useful, but ordering through PDF may be altered by the controller.

## 2. Conceptual model (keep these four concepts separate)

1. **Imported source layers:** immutable-by-default groups discovered in DXF, Gerber, SVG, vector PDF or manually created from selections. Carry input provenance, units, curves, artwork polarity and source colors. One input file may contain many source layers; multiple files may be registered into one document.
2. **Working layers:** user-managed, named derived geometries. A layer selects one or more source/working inputs and combines them through explicit boolean, offset, complement or clipping operations. A working layer has a geometry interpretation (filled region or linework) and may reference a boundary/domain. Dependencies form an acyclic graph. Layer list display order must **not** silently change boolean semantics.
3. **Pattern passes:** one or more independent geometric path-generation strategies applied to a working layer. Each pass specifies fill/trace pattern and numerical parameters, and maps its generated paths to an output group. A pass creates *geometry*, not a machine process or prescribed scan repetitions.
4. **Output color groups:** user-defined names and explicit RGB stroke colors. They gather passes from one or more layers into identifiable sets that Full Spectrum can separate manually. They carry PDF stroke width (global default with optional override) and geometrical path-order options, **not laser parameters**.

Use explicit stable IDs internally and user-editable display names. Duplicating a layer must duplicate its configuration, not source geometry blobs. A reference-only or boundary layer can be visible in preview without becoming a marking process.

## 3. User interface and workflow

Desktop-oriented single-page editor with:

- **Files/sources panel:** drag/drop, file picker, add/remove input, physical dimensions, units/scale/coordinate-frame verification, errors and unsupported geometry counts.
- **Layer panel:** tree/list of sources, working layers and pattern passes. Create/rename/duplicate/reorder/delete; toggle preview visibility, output enable and lock; select multiple; inspect source membership; choose geometry type and polarity. Show the dependency/usage relationship where a layer acts as a boundary or mask. Explicitly report missing/cyclic dependencies.
- **Properties panel:** for selected working layer, configure source selector, union/subtract/intersect/XOR, positive/negative geometry and domain; for a pass, configure pattern, pitch, angle, phase, offset, start strategy and output group.
- **Output groups panel:** assign/edit unique RGB colors, optional visibility, per-group PDF stroke width, group export enable; show collisions where two logically different groups share the same exact color.
- **Central preview:** pan/zoom/fit, ruler/mm coordinates, source artwork, resulting planar regions, fill toolpaths, color groups, marking direction and endpoints, hypothetical beam-off jumps and relative jump-overlap heatmap (toggleable). Distinguish exported strokes from preview-only lines visually.
- **Profiles toolbar:** choose built-in/saved profile, Save, Save As, Rename, Duplicate, Delete, Import JSON, Export JSON, Restore defaults; indicate unsaved edits and unmatched source bindings clearly.
- **Export panel:** PDF download, dimensions/page margin, optional separated per-group PDFs; SVG/DXF where implemented. Export exactly what selected/enabled passes produce, not hidden/reference geometry.

Changes update a dependency-aware preview and invalidate only downstream results where possible. Heavy operations use cancellable Web Workers. Never permit an older computation to replace a newer preview. Explicitly label preview-only hypothetical travel; do not leak it into production exports.

## 4. Input formats and geometry interpretation

Parsing is local and does not execute uploaded code, resolve external URLs, or render untrusted SVG markup directly into the DOM.

| Input | Minimum required behavior |
|---|---|
| **DXF** | Units; layers; LINE, ARC, CIRCLE, LWPOLYLINE/POLYLINE with bulges and nonzero widths; ELLIPSE/SPLINE approximate to a bounded tolerance; HATCH with holes; basic INSERT/block transforms. Detect unsupported REGION/ACIS rather than silently discard important features. |
| **Gerber RS-274X** | Units, strokes/flashes/arcs/regions, apertures, clear/dark polarity in source order, step-and-repeat and relevant attributes. Each file may map to a source layer. |
| **SVG** | Paths, standard shapes, fills/strokes, units/viewBox, grouping and nested transforms, fill rules, declared support for clipping. Warn/block unsupported masks/text/images/filters that affect output geometry. |
| **Vector PDF** | A documented supported vector subset: stroked paths, filled paths, transforms, colors and real page scale. User-selectable region-artwork vs trajectory-passthrough interpretation. Unsupported appearance features must not silently become raster approximations. |

Interpretation must distinguish **linework centerlines** from **filled regions**. A stroked open path is not a closed area just because an output laser process exists. Permit explicit stroke expansion to a region using original or user-specified width, with correct joins and caps. Region-only operations (boolean inversion, region fill) must diagnose unresolved open geometry; never close an open path implicitly.

Merge overlapping shapes where they represent a single area, preserve holes and source-compositing polarity, and avoid duplicate marking for overlapping CAD primitives unless the user deliberately makes multiple pattern passes. Preserve source coordinates and support explicit transform/alignment for multiple inputs. Always show source bounds versus derived bounds.

## 5. Layer composition and negative regions

Every working region is constructed through explicit, predictable operations. Required boolean operators are **union, intersection, subtraction, XOR and clipping**. For a negative working layer:

`negative_region = domain_region − subject_region`

Both operands must be **finite filled regions**, with exact hole/topology semantics. A negative region without a bounded domain is invalid and must produce a UI error, never an infinite plane or a guessed page rectangle.

### Domain providers (required)

| Provider | Definition and controls |
|---|---|
| **Named boundary layer** | Use filled interior of a selected closed contour/working region (including holes). May be an input mechanical outline or separately constructed mask. Reference-only by default. |
| **Axis-aligned bounding box** | Rectangle of a deliberately selected reference geometry set, expanded by a signed/unsigned configured margin in mm (normally positive). Do not infer the reference set from the current preview viewport. |
| **Bounding circle** | Smallest enclosing circle of the selected reference geometry (curves included within approximation tolerance), plus radial margin. |
| **Dilation / buffered geometry** | `buffer(reference_region, distance_mm)` with explicit joins. Can form a controlled halo around subject geometry if reference = subject. Requires positive distance to create a useful exterior band. |
| **Dilated convex hull** | `buffer(convexHull(reference_geometry), distance_mm)`. Smooth, compact overall envelope; default round buffer joins, optionally configurable. |

The user chooses the **reference geometry set** (named source/working layer(s), current subject, or explicitly all selected input artwork), the margin/dilation amount and any relevant join type. Show the generated boundary before running inversion. Handle disjoint outlines, nested holes, zero/negative margins and subject geometry falling outside the domain by issuing diagnostics. Optional future provider: rotated minimum-area bounding rectangle.

Reference layers **must not automatically produce mark paths**. A domain is geometry used by composition; the user can separately enable its own contour/marking pass if desired. Derived-layer references form a DAG: reject cycles. Evaluation order follows dependencies, not arbitrary visual layer ordering.

### Examples

- **Positive resist artwork:** expose copper as `union(TOP_LAYER)` with inward contours; show `ROUTE_TOOL_PATH` only as a separately colored alignment/outline layer.
- **Negative stencil/background:** `bbox(TOP_LAYER, +0.5 mm) − union(TOP_LAYER)`, filled with parallel hatch; pads remain holes.
- **Pattern limited to a mechanically defined part:** `part_outline − keepout_regions`, then fill with selected pattern.
- **Local border:** `dilate(subject, +0.5 mm) − subject`, producing a controlled ring instead of an arbitrary page-sized inverse.
- **Tight negative mask around arbitrary artwork:** `buffer(convexHull(subject), +0.5 mm) − subject`.

### Topology and numerical policy

Use a robust browser-available clipping/offset library (prefer **Clipper2 WASM**, or justify an equivalent). Define curve-flattening tolerance, integer scaling/range, winding/fill rules, winding orientation, open/closed treatment and deterministic output. Preserve small features; report topology changes induced by simplification/offsetting. No global hard-coded PCB assumptions.

## 6. Geometric trajectory generation

Patterns operate on a working region or linework, and yield ordered **marking centerlines**, each open or closed, with explicit group/layer provenance. Support multiple passes per working layer. A pattern's geometry stays separate from PDF's cosmetic stroke width and any assumed optical mark width.

### Mandatory v1 modes

| Mode | Operation | Controls |
|---|---|---|
| **Linework passthrough** | Preserve source centerline paths; suited to marking, vector engraving and cutting open paths. | Curve tolerance; optional simplification; direction/order; optional manual geometric offset. |
| **Boundary trace / contour-only** | Trace external and/or hole boundaries; optionally several offset perimeter rings. | Inside/on/outside offset, geometric kerf-compensation distance (user-entered, never assumed), ring count, join style, path direction; for cutting, inner features before outer. |
| **Inward contour fill** | Iterative region offsets from boundary toward interior; retain rings as closed paths and open centerline gap-fill for collapsed cores. | Pitch, first offset (default pitch/2), end spacing, narrow-feature gap-fill, simplifying tolerance. This is the default for dry-film LDI. |
| **Parallel hatch** | Equally spaced, finite scanlines clipped to region and holes. | Pitch, angle, phase/origin, unidirectional vs alternating/serpentine row ordering; multiple disjoint segments per row. |
| **Crosshatch / multi-angle hatch** | Independent parallel hatches at a user-editable list of angles. | Angle list (e.g. 0°, 90° or 0°/60°/120°), per-angle or shared pitch, phase, ordering; paths may overlap intentionally. |
| **Boundary + hatch** | Geometric perimeter shell(s) followed by parallel/crosshatch fill of remaining interior. | Shell count and spacing, hatch pitch/angles, optional inset between shell and infill. |

**Additional desirable patterns** (implement behind independently tested options, not by compromising v1 geometry): outward contour fill; safely linked continuous spiral following offsets within a single connected region; centerline/skeleton-only thin-feature engraving; configurable dot/stipple/hex lattice (as finite, importable tiny paths rather than zero-length “points”); variable density hatch; patterned dashes and gaps for perforation/tab features.

**Important details:**

- Offset/hatch line spacing is centerline distance, independent of export stroke width. First offset, phase and edge coverage have exact documented definitions.
- Repeated contour inward offsets must handle holes and concavities; each island remains separate unless a deliberate beam-on connector is geometrically safe and explicitly requested.
- Short open paths from medial-axis-like finishing are legitimate. Do not force closure or delete them as noise by default.
- For hatch, intersection with holes/keep-outs produces separate beam-on segments, with *no mark stroke connecting across voids*.
- If a spiral/connected fill inserts physical connecting strokes, show them as genuine **beam-on** geometry and audit their coverage; they are not invisible travel.
- Cutting-oriented boundary paths may support gaps/tabs/lead-ins in a later milestone; the v1 requirement is reliable boundary tracing and user-entered offset, not machine-specific tool compensation.
- Parametric angle is measured in physical document coordinates, not screen coordinates; phase may be locked to a global origin for adjacent layers to remain aligned.

## 7. Path ordering and travel-leakage optimization

The app has no authority over the laser's actual beam-off path. **Assume straight jumps between the emitted path endpoints only for prediction**, and label controller order and physical blanking behavior as unknown. Never output imaginary travel commands as marking lines.

Within each **output color group** (since vendor software may execute groups separately):

- Optimize route ordering, open-path reversal, and start seam/direction of closed loops while preserving mark geometry and any ordering constraints (e.g., holes before exterior for cutting).
- Provide modes: **as generated**, **minimum jump length**, and **bleed-aware overlap reduction**.
- Show geometric off-jump paths, total length, count, intersections and relative overlap/coverage distribution on user-selectable protected regions. Relative model can deposit a finite-width influence footprint per assumed jump; calculate peak, squared-overlap integral on keep-clear, total keep-clear coverage and jump length. Do not label this calibrated optical dose.
- Combine penalties to avoid optimizing a handful of hotspots by contaminating the entire image. Offer a deterministic nearest-endpoint baseline, then local improvement (seam choice, 2-opt-like moves, reversing, swaps) with an objective-before/after preview.
- Optional assumed start/end/return reference points only when provided by user; no invented center-of-field/home motion. Multiple independent color groups may have unknown execution order.
- Preserve intended sequence in PDF path-stream order, but **do not promise Full Spectrum executes in that order**. Supply a small optional ordering-verification test coupon.

Note: mere **chronological** rearrangement of an identical set of off-jumps does not reduce integrated exposure. The optimizer must change actual travel segment geometry/overlap, not only timestamps or list positions without changing adjacency.

## 8. Export behavior

### PDF — primary mandatory format

- 1:1 physical dimensions; mm → PDF points correctly (72 pt/in), explicit page margin independent of source coordinates. No fit-to-page or automatic scaling.
- Explicit paths, **nonzero RGB strokes**, no fills/raster artwork unless explicitly designing a separate export mode. Defaults: 0.0254 mm / 1 mil; optionally 0.05 mm. Exact stroke width is for the importer and **is not** optical exposure linewidth, kerf or fill pitch.
- One actual stroked path for each continuous mark trajectory; open stays open, loops closed once. Do not combine paths through holes with an exposed stroke. Numeric precision sufficient for ≤1 µm fixture tolerance.
- Each group has an exact RGB value; preserve color fidelity. Warn if two groups that should be independently processed use identical RGB.
- PDF process grouping is by **RGB color** for vendor import, not a PDF optional-content layer or embedded machine command. Source layers, working layers and output groups need not have a 1:1 correspondence. Optional export one PDF per group for importers that mishandle colors.
- Exclude all disabled/reference layers, domain outlines unless separately enabled, preview jump paths, guides, rulers, debug heatmaps and hidden construction objects.
- Output centerline order should match chosen planner order in the PDF content stream. Explicitly warn about possible reorder by laser software.
- Export metadata may name patterns/groups for the user's convenience but must not masquerade as laser process settings.

### Secondary output

Provide equivalent paths and physical scale in SVG and DXF if feasible; in DXF preserve named output groups/layers and true colors as supported. Do not require SVG/DXF to carry hypothetical travel, or emulate a machine-specific job file. Separate reference-only boundary export is useful.

## 9. Profile system — required

Profiles store **repeatable geometry-preparation rules**, including elaborate source-layer bindings, derived layer graph, inversions/boundaries, pattern passes, RGB output groups, export style, and geometric jump-optimization preferences.

- **Browser persistence:** local IndexedDB preferred for a growing named collection; localStorage acceptable for small JSON profiles. No login, sync server, or external database. Save/Save As/Rename/Duplicate/Delete; choose default/last-used profile. Show unsaved changes and provide explicit Reset. Keep source design files out of persistent storage unless the user explicitly asks for that separate future capability.
- **Portable JSON:** downloadable JSON and drag/drop/upload of profile JSON, validated against a **versioned schema** (`schemaVersion` + stable `profileId` + name). JSON is config data, never executable script or unrestricted expressions.
- **Schema migrations:** implement explicit version-to-version migrations and tests for old saved and imported profile versions. If a profile is from a newer unsupported schema, reject with a precise message and leave existing state unchanged. Import name/ID conflicts offer overwrite, import as copy, or cancel.
- **Reusable layer bindings:** reference source layers by ordered matching rules (format + exact layer name, optionally file-name pattern/source color, explicit user mapping). For Gerber one-file-per-layer mapping, support filename patterns. Regex if supported must be bounded/safe; avoid silent fuzzy guesses. Profiles must work on new designs with matching layer names without bundling the source design.
- **Partial/mismatched profile:** display mapping summary **matched / missing / ambiguous / extra**. Missing mandatory boundary layer or negative domain blocks that output until fixed, not silently substituted with an automatic bounding box. Optional layers may remain disabled/unresolved. Offer interactive remap and an explicit “update profile bindings” save.
- **Built-in read-only presets** to demonstrate PCB positive LDI, inverted/background hatch, general marking, and boundary cutting. A built-in may be duplicated/edited as a user profile.
- Profiles **do not store power, frequency, velocity, pulse width, physical process repeats or focus**. They do include user-selected geometry: hatch pitch, angle, spacing, colors, PDF strokes and geometric offsets. They also do not require storing drawings/projects or persistent UI sessions.
- Exporting the profile must round-trip to the same layer pipeline and output-group settings on a clean browser. Applying a profile is a reversible UI operation until explicitly saved.

### Proposed profile representation

A profile has `schemaVersion`, identity/name, `sourceBindings[]`, `workingLayers[]`, `outputGroups[]`, `optimizer`, `export`. Working layers reference stable IDs, list operands and boolean/inversion/domain definition; each contains zero or more `patternPasses[]` referencing an output group. Human-readable display names are not primary keys. Document the schema with a JSON Schema and ship representative test profiles. A sample profile is bundled; its naming/schema is illustrative and may be refined before implementation, but the semantics above are mandatory.

## 10. Diagnostics and safety of transformations

- Warn or fail clearly for unsupported curves/objects, unknown units, malformed geometry, empty regions, tiny uncovered gaps, open paths used as region boundaries, invalid offsets, self-intersections, holes lost by simplification, out-of-domain subject regions, collisions of output colors and missing profile bindings.
- Preview separate overlays for input geometry, domain, **result of boolean composition**, generated marking paths, and hypothetical non-marking jumps.
- Audit region fills against assumed swept effective marking width (user-set geometric surrogate), separately reporting spill, missed area and short disconnected segments. Do not derive actual UV dose or cutting capability without measurements.
- Deterministic runs with identical input/profile/library version. Stable path IDs/diagnostics where practical; tolerances reported in mm.
- No accidental duplicate output when multiple source primitives overlap; explicit independent passes may overlap intentionally and should be visible.

## 11. Browser-only implementation guidance

- TypeScript + Vite; Svelte/Preact/React or vanilla UI acceptable. Prefer a lean maintainable architecture, not a SPA framework as an end in itself.
- Parsers behind typed format adapters; canonical geometry model, dependency-evaluated working-region graph, polygon operations and path-generation strategies as independent modules; route planner; PDF/SVG/DXF exporters; versioned profile manager. A testable **core library separate from UI components** is essential.
- WASM or tested pure JS geometry kernel for robust boolean and offsets (Clipper2 WASM preferred); use bounded-error curve approximation. Canvas/WebGL/SVG preview must not become the authoritative geometric data source.
- Web Workers for computationally expensive operations, progress/cancellation and revision-tagged updates. Cached DAG results for repeated profile tweaks. Avoid unbounded memory for many fine-pitch paths.
- Static deploy with bundled JS/WASM and no runtime CDN or telemetry. Complete an import-to-PDF workflow with network disconnected after initial load. Source files never leave the device.
- Developer-only commands: `just dev`, `just test`, `just build`, `just regression`; a pinned Nix shell/build is desirable. The **end user uses the website**, not a local CLI.
- Treat uploaded SVG/PDF/JSON as untrusted: no JS eval, unsafe HTML injection, external resource fetches, arbitrary filesystem access, or unbounded decompression/regex execution.

## 12. Regression fixtures and acceptance tests

Retain the existing `display-pogo-board.DXF`, source PDF, legacy generation script/audit as **historical references**, and the known-good 10 µm / 20 µm blue-copper/red-outline PDF files as import/export regressions. The legacy fixture uses `TOP_LAYER` for copper and `ROUTE_TOOL_PATH` for mechanical border. Reported board outline approximately **25.0 × 11.3 mm**; copper width **24.130 mm**; 10 copper islands. The old PDFs have 660/835 copper trajectories plus one outline; improved algorithms may produce different counts if the geometry and coverage are correct. **Do not preserve past mistakes** such as missing outlines or zero-width strokes.

Mandatory automated tests:

1. Static site completes file import → layer mapping → pattern generation → export PDF offline after initial load; source remains local.
2. Source DXF preserves physical bounds, copper and border as separate layers; no silent loss of important HATCH/REGION content.
3. Source linework and closed filled regions retain distinct semantics; stroked open linework is never accidentally treated as closed mask.
4. Positive copper contour fill at 10/20 µm, including narrow open centerline finish, preserves islands and holes at 1:1 scale.
5. Synthetic nested/disconnected polygons correctly implement domain subtraction with a named boundary, preserving hole structure.
6. The **same subject** can be inverted against box, circle, buffered subject and buffered convex hull. Geometry must be bounded and recomputed after changing source/parameter.
7. Missing/cyclic boundaries are rejected, not guessed. A reference boundary is never automatically emitted as a marking stroke.
8. Parallel hatch clips exactly at subject and hole boundaries; angle and phase behave in mm coordinates; no unwanted strokes connecting separate fragments.
9. Crosshatch at two angles produces independent passes; perimeter+hatch does not silently expose outside target region.
10. Boundary tracing handles internal holes, offset side and inside-before-outside sequence constraints.
11. Reordering layers in UI does not change derived regions unless explicit operations change. Mutating an upstream region invalidates only dependent outputs.
12. Multiple passes targeting the same RGB output group and different groups export with precise finite stroke widths and exact colors; same RGB collisions are warned about.
13. PDF 0.0254 → 0.05 mm stroke change preserves centerlines, fill pitch, order and scale. Never emit unintended filled ribbons.
14. Hypothetical off-jumps/heatmaps never appear in the production PDF. A synthetic overlapping-jumps scenario improves relative bleed objective without modifying mark paths.
15. Trajectory-passthrough of the known-good PDFs preserves open/closed paths and their RGB groups.
16. Export/reimport of a named profile recovers all layer bindings, operations, fill patterns, colors and geometry parameters. Old schema migrates; future schema fails safely.
17. Applying a profile to new files with missing/ambiguous layer mappings requires explicit resolution; mandatory domain references may not be silently remapped.
18. Saving and reloading profiles works without a server; no source file bytes are persisted by default. Reset/delete behaves predictably.
19. Heavy geometry computation is cancellable; stale worker results cannot overwrite newer profile revisions.
20. Preview distinguishes working/negative domains, beam-on paths and speculative jump paths without changing production export.

## 13. Milestones

**M1 — functioning browser-only vertical slice:** DXF fixture import with proper layers; source/working/output-group distinction; simple layer panel; contour fill and passthrough; correct finite-width color PDF export; basic profile save/import/export; offline-after-load support. Validate with actual fixture.

**M2 — geometry and layers:** Full dependency graph and layer operations, explicit negative domains (all five providers), booleans, reliable topology, named boundary handling, cutting offsets; parallel and crosshatch fills; per-layer pattern passes; profile binding diagnostics/migrations. Demonstrate synthetic negative and multi-layer projects.

**M3 — breadth and optimization:** Gerber/SVG and documented vector PDF subset; boundary+hatch, gap fill refinements, bleed-aware ordering with route visualization, inside-before-outside constraints; SVG/DXF export where supported.

**M4 — robustness/release:** Geometric accuracy auditing, fuzz/regression testing, performance/cancellation, UI polish, large files, cross-browser tests in Chromium/Firefox, static deployment documentation and packaged build.

**Potential post-v1 enhancements:** safe spiral linking, stipple lattices, perforation/tab toolpaths, manual geometry editing, alternate envelope shapes and importer-specific compatibility presets. Explicitly defer any machine protocol/control integration.

## 14. Explicit non-goals

No laser control, machine-specific G-code/cut file, transferring machine/process settings via PDF, beam modulation planning, optical dose/thermal model without actual calibration, backend services, account or cloud sync, source-file persistence without consent, raster-image engraving pipeline, 3D slicing, CAD dimensioning/modeling, or general-purpose vector illustration editing. Keep this a strong **2D laser vector preparation tool**, not another bloated CAD package.
