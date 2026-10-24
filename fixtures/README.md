# Regression fixtures

Inputs and known-good outputs referenced by `docs/spec.md` §12 and exercised by
`just regression`. Expected contents (not yet checked in):

- `display-pogo-board.DXF` — legacy board, layers `TOP_LAYER` (copper) and
  `ROUTE_TOOL_PATH` (mechanical outline). Outline ≈ 25.0 × 11.3 mm, copper width
  24.130 mm, 10 copper islands.
- Source PDF and the legacy generation script / audit, as historical references.
- Known-good 10 µm and 20 µm contour-fill PDFs (blue copper, red outline, 0.0254 mm
  strokes, 1:1) for import/export regressions.
- A sample profile JSON.

Fixtures are test data only: never bundled into the shipped site.
