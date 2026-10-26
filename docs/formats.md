# Supported file formats

What each reader understands, what it reports instead of reading, and what each exporter
writes. Everything is converted to millimetres on import. Anything outside a reader's
subset is reported in the Diagnostics panel; nothing is dropped silently. Requirements are
in [`spec.md`](spec.md) §4 and §8; design choices in
[`engineering/decisions.md`](engineering/decisions.md) (D14, D22, D23).

## Curves

The geometry model holds straight segments and exact circular arcs. Arcs stay exact from
import to DXF and SVG export, and become Bézier curves within 1 µm in PDF export; the
preview draws them as arcs at any zoom. Other curves (ellipses,
splines, Béziers that are not circular) are flattened at import within the read tolerance
of 1 µm. A Bézier counts as a circular arc when its end tangents are symmetric about the
chord and it stays within 1 µm, or within 0.03 % of the radius, of the circle; 0.03 % is the
error the usual Bézier approximation of an arc itself makes, so a circle drawn as Béziers
comes back as the circle it approximates.

## How a file becomes source layers

The format is detected from the file's content first and its extension second (`.xml` may
be anything, `.gm1` is only a convention). Every reader produces named source layers of two
kinds of primitive:

- **Paths**: open or closed centrelines, optionally with a constant width (polyline widths,
  Gerber draws, stroked SVG/PDF paths, GDSII and IPC-2581 round-ended paths).
- **Filled areas**: loops with a fill rule (hatches, flashes, regions, filled paths,
  polygons).

Two safeguards apply to every format:

- **Size limit.** Nested blocks, arrays, step-and-repeat, references and forms can make a
  small file expand enormously. An import may produce about 10 million vertices (roughly
  240 MB, a quarter of what the browser's geometry worker can address); a file that
  expands past that is refused with an error rather than imported in part.
- **Coordinates beyond 1 km** from the origin, or not finite, mark a corrupt or
  mis-scaled file. Those primitives are dropped with an error naming the layer.

Composited images (Gerber and IPC-2581 polarity, PDF and SVG clipping) whose boundaries
come within a few nanometres of each other can occasionally not be resolved completely;
the reader then warns that part of the layer was left out instead of dropping it
silently.

A layer may also carry a declared **function** (Gerber X2 `TF.FileFunction`, IPC-2581
`layerFunction,side`), which profile slots can match with a `function` rule.

## DXF (ASCII, R12 and later)

| Read | Notes |
|---|---|
| LINE, ARC, CIRCLE | Arcs stay exact. |
| LWPOLYLINE, 2D POLYLINE | Bulges as arcs; constant widths kept as path widths. |
| ELLIPSE, SPLINE | Flattened within the read tolerance (1 µm). SPLINE by de Boor evaluation. |
| HATCH | Polyline and edge boundaries (lines, arcs, ellipses, splines), nesting styles. |
| SOLID, TRACE | Filled quadrilaterals. |
| REGION | Planar regions from the ACIS text DXF R13 to R2010 embed (SAT 1.x to 7.x): each face a filled area, straight and circular edges exact, elliptic ones flattened. Regions DXF 2013 and later store in binary (ACDSDATA) are reported, not read. |
| Outlines in pieces | Zero-width lines, arcs and open polylines of one layer that meet end to end in a closed cycle (every joint shared by exactly two pieces) become one closed outline, as CAD often draws pads and cut-outs (info). Open chains and junctions are left as they are. |
| INSERT | Blocks expanded with scale, rotation and translation; entities on layer 0 inherit the insert's layer. Non-uniform scale flattens arcs (warning). Depth limited to 32. |
| `$INSUNITS` | in, mm, cm, m, mil, µm. Unitless files use the assumed unit (mm), reported. |

Reported instead of read: BODY, 3DSOLID, SURFACE and proxy entities, and REGIONs that
cannot be read (error: ACIS geometry, explode it in CAD), MESH and polyface or 3D POLYLINE (warning), TEXT, MTEXT,
attributes, DIMENSION and leaders (info), unknown entities (warning). Binary DXF is refused.

## Gerber RS-274X and X2

One layer per file, named after the file.

| Read | Notes |
|---|---|
| `%FS` `%MO` | Leading or trailing zero omission, absolute or incremental, mm or in. |
| Apertures C, R, O, P | With optional round holes. |
| Aperture macros | Variables, arithmetic, primitives 1, 2/20, 4, 5, 6, 7, 21, 22, exposure off. |
| D01 / D02 / D03 | Draws are chained into polylines; a chain that returns to its start is a closed contour. |
| G01 / G02 / G03, G74 / G75 | Single- and multi-quadrant arcs; full circles. |
| G36 / G37 regions | With arcs. |
| `%LP` polarity | Dark and clear composited in source order into one region. |
| `%SR`, `%AB` | Step-and-repeat; aperture blocks with nested polarity. |
| `%LM` `%LR` `%LS` | Mirroring, rotation and scaling of flashed apertures. |
| `%TF.FileFunction` | Becomes the layer function (`Copper,L1,Top`, `Profile,NP`…). |

Reported: draws with non-circular apertures (approximated by a round stroke of the
aperture's extent), deprecated image transforms (OF, SF, MI, AS, IR, IPNEG; not applied),
missing units or end of file.

## SVG

| Read | Notes |
|---|---|
| Units | Root `width`/`height` in mm, cm or in with the viewBox mapping; unitless or px uses 96 px per inch (reported). |
| Layers | Each named top-level group is a layer, named by its Inkscape label or id; loose elements go to a layer named after the file. |
| Shapes and paths | All SVG shapes and path commands. Curves that are circular arcs (circles, rounded rectangle corners, circular arc commands) are read as exact arcs; other curves (ellipses, general Béziers) are flattened within the read tolerance. |
| `use`, `symbol`, CSS, nested transforms | Resolved by usvg. |
| Fill | Filled loops with the SVG fill rule. Gradient and pattern paints are read as solid (info). |
| Stroke | Centrelines with the stroke width scaled by the transform; a path that ends where it starts is a closed loop. Caps and joins are always expanded round (non-round ones reported). |
| Unpainted paths | Centrelines without width (info). |
| `clipPath` | Applied exactly to fills; stroked and unpainted paths inside a clip are read unclipped (warning). |

Reported instead of read: text (error: convert text to paths first), images, masks,
filters, `foreignObject` (warnings).

## Vector PDF

| Read | Notes |
|---|---|
| Pages | Every page; layer names get a `pN ` prefix when there are several. |
| Units | Points (1/72 in), times `/UserUnit`; the MediaBox lower-left corner is the origin. |
| Path construction | `m l c v y h re`. Bézier curves that are circular arcs are read as exact arcs; other curves are flattened within the read tolerance. |
| Painting | `S s f F f* B B* b b* n`; fills with their rule, strokes with the scaled line width. A stroked path that ends where it starts is a closed loop. |
| Graphics state | `q Q cm w d`; dash patterns are read solid (info). |
| Colour | `g G rg RG k K sc SC scn SCN` in gray, RGB and CMYK. |
| Clipping | `W W*` applied exactly to fills; strokes inside a clip are read unclipped (warning). |
| Form XObjects | With their matrix and bounding box; nested up to 16 deep. |
| Layers | The optional-content group's name (`/OC … BDC`) when present, otherwise the paint colour as `#rrggbb`. A colour-separated PDF therefore imports as one layer per process colour. |

Reported instead of read: text (error), images, shadings (warnings), pattern paints (read
as solid), `/Rotate` (ignored), forms that invoke themselves (error, the cyclic invocation
is skipped). Encrypted PDFs are refused. Decompressed content is limited
to 256 MiB per page.

## GDSII

| Read | Notes |
|---|---|
| UNITS | Database unit to millimetres; the user unit is reported. |
| BOUNDARY, BOX | Filled polygons. |
| PATH | Flush (0), square (2) and custom (4) ends become exact polygons with mitred joins; round ends (1) stay centrelines with their width. |
| SREF, AREF | Flattened with reflection, magnification and rotation; nesting limited to 64 levels. |
| Layers | `layer/datatype`; prefixed with the structure name when the library has several top structures. |

Reported: TEXT and NODE (info), missing referenced structures (error).

## IPC-2581 (revisions B and C)

| Read | Notes |
|---|---|
| Units | `CadHeader` units (mm, in, µm, mil). |
| Layers | Each `LayerFeature` is a layer; its function is `layerFunction,side` (for example `CONDUCTOR,TOP`, or `SIGNAL,INTERNAL` for an inner layer, which several layers may share). The board profile is a layer named `Profile` with function `PROFILE`. Files with the `.cvg` extension (Altium's IPC-2581 export) are recognised by their content. |
| Standard primitives | Circle, RectCenter, RectCorner, RectRound, RectCham, Oval, Donut, Octagon, Hexagon, Diamond, Triangle, Ellipse, Thermal, Contour, Polygon. |
| User primitives | Polygons, contours and lines inside `UserSpecial`. |
| Line descriptors | By reference (`LineDescRef`) or written inline (`LineDesc`, as Altium writes them). Round ends stay centrelines with width; square and flat ends become exact polygons. |
| Features | Pads with `Xform` and `Location`, lines, arcs, polylines, contours, holes, standard primitives placed directly (after an `Xform` and `Location`), and `UserSpecial` groups of these (Altium writes a layer's traces this way). |
| Padstacks | Each `LayerPad` of a step's `PadStack`s is a pad on its layer (shape referenced or inline). Altium writes most pads only there; a pad that the layer's features also carry (same place, rotation and primitive) is read once. `LayerHole` is not read. |
| Polarity | Positive and negative sets composited in source order. |
| Steps | Several steps prefix layer names with the step name. |

Reported: unsupported primitives such as Butterfly, Moire, Text and Marking (warning),
references to missing primitives (warning), component placements (info, not geometry).

## Exports

All exports carry exactly what enabled passes produced for exporting output groups, in
the planned path order. Reference layers, envelopes, preview jumps and heatmaps are never
exported. Stroke widths are importer hints, not exposure or kerf.

| Format | Content |
|---|---|
| PDF (mandatory) | PDF 1.4, deterministic. 1:1 page around the paths plus the margin, stroked paths only, one exact RGB per output group, stroke width per group (default 0.0254 mm). PDF has no arc operator, so arcs are written as cubic Bézier curves, split so each stays within the curve tolerance (default 1 µm) of the arc. Optionally one file per group on an identical page so the files register. |
| SVG | Same page as the PDF, millimetre units with a 1:1 viewBox, one Inkscape layer per group with the group's colour and stroke width, round caps and joins. Arcs are exact SVG arc commands. |
| DXF | R2000, model coordinates (no page offset), one layer per group with the nearest AutoCAD colour index, LWPOLYLINEs with bulges so arcs stay arcs. Layer names are made DXF-safe. |
