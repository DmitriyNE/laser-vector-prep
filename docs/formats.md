# Supported file formats

What each reader understands, what it reports instead of reading, and what each exporter
writes. Everything is converted to millimetres on import. Anything outside a reader's
subset is reported in the Diagnostics panel; nothing is dropped silently. Requirements are
in [`spec.md`](spec.md) §4 and §8; design choices in
[`engineering/decisions.md`](engineering/decisions.md) (D14, D22, D23).

## How a file becomes source layers

The format is detected from the file's content first and its extension second (`.xml` may
be anything, `.gm1` is only a convention). Every reader produces named source layers of two
kinds of primitive:

- **Paths**: open or closed centrelines, optionally with a constant width (polyline widths,
  Gerber draws, stroked SVG/PDF paths, GDSII and IPC-2581 round-ended paths).
- **Filled areas**: loops with a fill rule (hatches, flashes, regions, filled paths,
  polygons).

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
| INSERT | Blocks expanded with scale, rotation and translation; entities on layer 0 inherit the insert's layer. Non-uniform scale flattens arcs (warning). Depth limited to 32. |
| `$INSUNITS` | in, mm, cm, m, mil, µm. Unitless files use the assumed unit (mm), reported. |

Reported instead of read: REGION, BODY, 3DSOLID, SURFACE and proxy entities (error: ACIS
geometry, explode it in CAD), MESH and polyface or 3D POLYLINE (warning), TEXT, MTEXT,
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
| Shapes and paths | All SVG shapes and path commands; Béziers flattened within the read tolerance. |
| `use`, `symbol`, CSS, nested transforms | Resolved by usvg. |
| Fill | Filled loops with the SVG fill rule. Gradient and pattern paints are read as solid (info). |
| Stroke | Centrelines with the stroke width scaled by the transform. Caps and joins are always expanded round (non-round ones reported). |
| Unpainted paths | Centrelines without width (info). |
| `clipPath` | Applied exactly to fills; stroked and unpainted paths inside a clip are read unclipped (warning). |

Reported instead of read: text (error: convert text to paths first), images, masks,
filters, `foreignObject` (warnings).

## Vector PDF

| Read | Notes |
|---|---|
| Pages | Every page; layer names get a `pN ` prefix when there are several. |
| Units | Points (1/72 in), times `/UserUnit`; the MediaBox lower-left corner is the origin. |
| Path construction | `m l c v y h re`, flattened within the read tolerance. |
| Painting | `S s f F f* B B* b b* n`; fills with their rule, strokes with the scaled line width. |
| Graphics state | `q Q cm w d`; dash patterns are read solid (info). |
| Colour | `g G rg RG k K sc SC scn SCN` in gray, RGB and CMYK. |
| Clipping | `W W*` applied exactly to fills; strokes inside a clip are read unclipped (warning). |
| Form XObjects | With their matrix and bounding box; nested up to 16 deep. |
| Layers | The optional-content group's name (`/OC … BDC`) when present, otherwise the paint colour as `#rrggbb`. A colour-separated PDF therefore imports as one layer per process colour. |

Reported instead of read: text (error), images, shadings (warnings), pattern paints (read
as solid), `/Rotate` (ignored). Encrypted PDFs are refused. Decompressed content is limited
to 256 MiB per page.

## GDSII

| Read | Notes |
|---|---|
| UNITS | Database unit to millimetres; the user unit is reported. |
| BOUNDARY, BOX | Filled polygons. |
| PATH | Flush (0), square (2) and custom (4) ends become exact polygons with mitred joins; round ends (1) stay centrelines with their width. |
| SREF, AREF | Flattened with reflection, magnification and rotation; nesting limited to 64 levels and 5 million placements. |
| Layers | `layer/datatype`; prefixed with the structure name when the library has several top structures. |

Reported: TEXT and NODE (info), missing referenced structures (error).

## IPC-2581 (revisions B and C)

| Read | Notes |
|---|---|
| Units | `CadHeader` units (mm, in, µm, mil). |
| Layers | Each `LayerFeature` is a layer; its function is `layerFunction,side` (for example `CONDUCTOR,TOP`). The board profile is a layer named `Profile` with function `PROFILE`. |
| Standard primitives | Circle, RectCenter, RectCorner, RectRound, RectCham, Oval, Donut, Octagon, Hexagon, Diamond, Triangle, Ellipse, Thermal, Contour, Polygon. |
| User primitives | Polygons, contours and lines inside `UserSpecial`. |
| Line descriptors | Round ends stay centrelines with width; square and flat ends become exact polygons. |
| Features | Pads with `Xform` and `Location`, lines, arcs, polylines, contours, holes. |
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
| PDF (mandatory) | PDF 1.4, deterministic. 1:1 page around the paths plus the margin, stroked paths only, one exact RGB per output group, stroke width per group (default 0.0254 mm). Optionally one file per group on an identical page so the files register. |
| SVG | Same page as the PDF, millimetre units with a 1:1 viewBox, one Inkscape layer per group with the group's colour and stroke width, round caps and joins. |
| DXF | R2000, model coordinates (no page offset), one layer per group with the nearest AutoCAD colour index, LWPOLYLINEs with bulges so arcs stay arcs. Layer names are made DXF-safe. |
