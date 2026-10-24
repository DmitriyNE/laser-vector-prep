//! Synthetic regression fixtures, generated deterministically from `lvp_core::dxf::write`.
//!
//! Each fixture documents what a correct importer/geometry pipeline must see in it
//! (`expect`), so tests do not depend on hand-counted numbers hidden in test code.
//! Regenerate with `just fixtures`; `tests/up_to_date.rs` fails when fixtures/ is stale.

use std::collections::BTreeMap;

pub mod gerber;

use lvp_core::dxf::write::{Document, Entity, LwVertex, Placed};
use lvp_core::geom::Point;
use lvp_core::units::Unit;
use serde_json::{Map, Value, json};

pub const MANIFEST_FILE: &str = "manifest.json";

/// Bulge of a 90° counter-clockwise arc (`tan(90° / 4)`).
const QUARTER: f64 = 0.414_213_562_373_095_1;
/// Bulge of a 180° counter-clockwise arc.
const HALF: f64 = 1.0;

pub struct Fixture {
    pub name: &'static str,
    pub description: &'static str,
    pub document: Document,
    /// What a correct pipeline must observe, in millimetres. Keys are fixture-specific.
    pub expect: Value,
    pub notes: Vec<&'static str>,
}

impl Fixture {
    pub fn file_name(&self) -> String {
        format!("dxf/{}.dxf", self.name)
    }
}

fn pt(x: f64, y: f64) -> Point {
    Point::new(x, y)
}

fn circle(center: Point, radius: f64) -> Entity {
    Entity::Circle { center, radius }
}

fn polyline(vertices: Vec<LwVertex>, closed: bool, const_width: Option<f64>) -> Entity {
    Entity::LwPolyline {
        vertices,
        closed,
        const_width,
    }
}

fn line(a: Point, b: Point) -> Entity {
    Entity::Line { start: a, end: b }
}

/// Rounded rectangle as one closed LWPOLYLINE with bulge corners.
fn rounded_rect(min: Point, max: Point, r: f64) -> Entity {
    polyline(
        vec![
            LwVertex::new(min.x + r, min.y),
            LwVertex::bulged(max.x - r, min.y, QUARTER),
            LwVertex::new(max.x, min.y + r),
            LwVertex::bulged(max.x, max.y - r, QUARTER),
            LwVertex::new(max.x - r, max.y),
            LwVertex::bulged(min.x + r, max.y, QUARTER),
            LwVertex::new(min.x, max.y - r),
            LwVertex::bulged(min.x, min.y + r, QUARTER),
        ],
        true,
        None,
    )
}

/// Board-like artwork replacing the historical pogo-board fixture: outline plus copper islands.
fn board_islands() -> Fixture {
    let mut d = Document::new(Unit::Millimetre);
    d.layer("TOP_LAYER", 5).layer("ROUTE_TOOL_PATH", 1);

    d.add(
        "ROUTE_TOOL_PATH",
        rounded_rect(pt(0.0, 0.0), pt(25.0, 11.3), 1.0),
    );

    // Two rows of three round pads; the leftmost pair is joined by a trace (one island).
    for &x in &[3.0, 6.0, 9.0] {
        d.add("TOP_LAYER", circle(pt(x, 2.8), 0.6));
        d.add("TOP_LAYER", circle(pt(x, 8.5), 0.6));
    }
    d.add(
        "TOP_LAYER",
        polyline(
            vec![LwVertex::new(3.0, 3.3), LwVertex::new(3.0, 8.0)],
            false,
            Some(0.3),
        ),
    );
    // Two rectangular pads.
    d.add("TOP_LAYER", Entity::rect(pt(15.0, 2.0), pt(17.0, 4.0)));
    d.add("TOP_LAYER", Entity::rect(pt(15.0, 7.3), pt(17.0, 9.3)));
    // Obround slot pad with semicircular ends.
    d.add(
        "TOP_LAYER",
        polyline(
            vec![
                LwVertex::bulged(11.0, 5.15, 0.0),
                LwVertex::bulged(13.0, 5.15, HALF),
                LwVertex::bulged(13.0, 6.15, 0.0),
                LwVertex::bulged(11.0, 6.15, HALF),
            ],
            true,
            None,
        ),
    );
    // Free-standing zigzag trace (open, constant width): an island on its own.
    d.add(
        "TOP_LAYER",
        polyline(
            vec![
                LwVertex::new(14.0, 5.0),
                LwVertex::new(15.0, 6.3),
                LwVertex::new(16.0, 5.0),
                LwVertex::new(17.0, 6.3),
                LwVertex::new(18.0, 5.0),
            ],
            false,
            Some(0.25),
        ),
    );
    // Annulus: outer circle with a concentric inner circle = one island with one hole.
    d.add("TOP_LAYER", circle(pt(21.0, 5.65), 1.5));
    d.add("TOP_LAYER", circle(pt(21.0, 5.65), 0.8));

    Fixture {
        name: "board-islands",
        description: "Board outline (ROUTE_TOOL_PATH) and copper artwork (TOP_LAYER) with pads, traces, a slot and an annulus.",
        document: d,
        expect: json!({
            "outline_size_mm": [25.0, 11.3],
            "copper_bounds_mm": [2.4, 2.0, 22.5, 9.3],
            "copper_width_mm": 20.1,
            "copper_islands": 10,
            "copper_holes": 1,
            "trace_widths_mm": [0.3, 0.25],
        }),
        notes: vec![
            "copper_bounds_mm includes circle radii but not polyline widths (the widest trace stays inside the pads)",
            "the trace on x = 3 overlaps both pads it joins; a union must count them as one island",
            "the inner circle of the annulus is nested in the outer one and must become a hole, not a second island",
        ],
    }
}

/// Nested regions and a disconnected part, for domain-subtraction and hole-topology tests.
fn nested_holes() -> Fixture {
    let mut d = Document::new(Unit::Millimetre);
    d.layer("SHAPES", 3);
    d.add("SHAPES", Entity::rect(pt(0.0, 0.0), pt(20.0, 20.0)));
    d.add("SHAPES", Entity::rect(pt(4.0, 4.0), pt(16.0, 16.0)));
    d.add("SHAPES", circle(pt(10.0, 10.0), 3.0));
    d.add("SHAPES", Entity::rect(pt(9.0, 9.0), pt(11.0, 11.0)));
    d.add(
        "SHAPES",
        polyline(
            vec![
                LwVertex::new(25.0, 0.0),
                LwVertex::new(31.0, 0.0),
                LwVertex::new(28.0, 5.0),
            ],
            true,
            None,
        ),
    );
    d.add("SHAPES", circle(pt(28.0, 10.0), 1.0));
    Fixture {
        name: "nested-holes",
        description: "Square with a square hole containing a circular island with its own square hole, plus a disjoint triangle and circle.",
        document: d,
        expect: json!({
            "closed_contours": 6,
            "top_level_regions": 3,
            "holes": 2,
            "nesting_depth": 4,
            "filled_area_mm2": 400.0 - 144.0 + (std::f64::consts::PI * 9.0) - 4.0 + 15.0 + std::f64::consts::PI,
        }),
        notes: vec![
            "even-odd and nonzero-with-orientation must agree here: every contour is a simple loop",
            "the triangle and the small circle are separate top-level regions",
        ],
    }
}

/// Open linework that must never be treated as a region, next to one genuine region.
fn open_linework() -> Fixture {
    let mut d = Document::new(Unit::Millimetre);
    d.layer("MARK", 2).layer("FILL", 5);
    d.add("MARK", line(pt(0.0, 0.0), pt(10.0, 0.0)));
    d.add(
        "MARK",
        Entity::Arc {
            center: pt(15.0, 5.0),
            radius: 5.0,
            start_deg: 180.0,
            end_deg: 360.0,
        },
    );
    d.add(
        "MARK",
        polyline(
            vec![
                LwVertex::new(0.0, 10.0),
                LwVertex::new(2.0, 12.0),
                LwVertex::new(4.0, 10.0),
                LwVertex::new(6.0, 12.0),
                LwVertex::new(8.0, 10.0),
            ],
            false,
            None,
        ),
    );
    // Three and a half sides of a square: a 0.5 mm gap keeps it open.
    d.add("MARK", line(pt(20.0, 0.0), pt(30.0, 0.0)));
    d.add("MARK", line(pt(30.0, 0.0), pt(30.0, 10.0)));
    d.add("MARK", line(pt(30.0, 10.0), pt(20.0, 10.0)));
    d.add("MARK", line(pt(20.0, 10.0), pt(20.0, 0.5)));
    d.add("FILL", Entity::rect(pt(0.0, -5.0), pt(10.0, -1.0)));
    Fixture {
        name: "open-linework",
        description: "Lines, an arc, an open polyline and an almost-closed square on MARK; one closed rectangle on FILL.",
        document: d,
        expect: json!({
            "MARK": { "open_paths": 7, "closed_contours": 0, "gap_mm": 0.5 },
            "FILL": { "open_paths": 0, "closed_contours": 1 },
        }),
        notes: vec![
            "a region operation on MARK must fail with a diagnostic naming the open geometry, never auto-close",
            "the arc is the lower half of a circle; its bounds reach y = 0",
        ],
    }
}

/// Same shapes as a millimetre file would hold, but authored in inches.
fn units_inch() -> Fixture {
    let mut d = Document::new(Unit::Inch);
    d.layer("PART", 7);
    d.add("PART", Entity::rect(pt(0.0, 0.0), pt(1.0, 0.5)));
    d.add("PART", circle(pt(0.5, 0.25), 0.1));
    Fixture {
        name: "units-inch",
        description: "$INSUNITS = 1 (inches): a 1 x 0.5 in rectangle with a 0.1 in radius circle.",
        document: d,
        expect: json!({
            "insunits": 1,
            "bounds_mm": [0.0, 0.0, 25.4, 12.7],
            "circle_radius_mm": 2.54,
        }),
        notes: vec!["importers must convert on read; the UI must show 25.4 x 12.7 mm, not 1 x 0.5"],
    }
}

/// Block references with translation, rotation and scale.
fn blocks_insert() -> Fixture {
    let mut d = Document::new(Unit::Millimetre);
    d.layer("PADS", 4);
    d.block(
        "PAD",
        vec![
            Placed {
                layer: "0".into(),
                entity: circle(pt(0.0, 0.0), 0.5),
            },
            Placed {
                layer: "0".into(),
                entity: Entity::rect(pt(-1.0, -1.0), pt(1.0, 1.0)),
            },
        ],
    );
    let insert = |at: Point, scale: f64, rotation_deg: f64| Entity::Insert {
        block: "PAD".into(),
        at,
        scale,
        rotation_deg,
    };
    d.add("PADS", insert(pt(0.0, 0.0), 1.0, 0.0));
    d.add("PADS", insert(pt(10.0, 0.0), 1.0, 45.0));
    d.add("PADS", insert(pt(0.0, 10.0), 2.0, 0.0));
    d.add("PADS", insert(pt(10.0, 10.0), 0.5, 90.0));
    let s2 = 2f64.sqrt();
    Fixture {
        name: "blocks-insert",
        description: "One block (circle + square) inserted four times: identity, rotated 45 deg, scaled 2x, scaled 0.5x and rotated 90 deg.",
        document: d,
        expect: json!({
            "inserts": 4,
            "resolved_closed_contours": 8,
            "bounds_mm": [-2.0, -s2, 10.0 + s2, 12.0],
            "insert_bounds_mm": [
                [-1.0, -1.0, 1.0, 1.0],
                [10.0 - s2, -s2, 10.0 + s2, s2],
                [-2.0, 8.0, 2.0, 12.0],
                [9.5, 9.5, 10.5, 10.5],
            ],
        }),
        notes: vec![
            "block entities sit on layer 0 and must inherit the INSERT's layer (PADS)",
            "order of operations is scale, then rotate, then translate",
        ],
    }
}

pub fn all() -> Vec<Fixture> {
    vec![
        board_islands(),
        nested_holes(),
        open_linework(),
        units_inch(),
        blocks_insert(),
    ]
}

fn count_entities(placed: &[Placed]) -> BTreeMap<String, BTreeMap<&'static str, usize>> {
    let mut by_layer: BTreeMap<String, BTreeMap<&'static str, usize>> = BTreeMap::new();
    for p in placed {
        *by_layer
            .entry(p.layer.clone())
            .or_default()
            .entry(p.entity.type_name())
            .or_insert(0) += 1;
    }
    by_layer
}

fn bbox_json(b: Option<lvp_core::geom::BBox>, to_mm: &dyn Fn(f64) -> f64) -> Value {
    match b {
        Some(b) => json!([
            to_mm(b.min.x),
            to_mm(b.min.y),
            to_mm(b.max.x),
            to_mm(b.max.y)
        ]),
        None => Value::Null,
    }
}

fn unit_name(u: Unit) -> &'static str {
    match u {
        Unit::Millimetre => "mm",
        Unit::Micrometre => "um",
        Unit::Centimetre => "cm",
        Unit::Metre => "m",
        Unit::Inch => "in",
        Unit::Mil => "mil",
    }
}

/// The manifest describing every fixture: file, units, per-layer entity counts, computed
/// bounds in mm and the hand-specified expectations.
pub fn manifest(fixtures: &[Fixture]) -> Value {
    let entries: Vec<Value> = fixtures
        .iter()
        .map(|f| {
            let d = &f.document;
            let to_mm = |v: f64| d.units.to_mm(v);
            let mut layers = Map::new();
            for (layer, counts) in count_entities(&d.entities) {
                layers.insert(
                    layer.clone(),
                    json!({
                        "entities": counts,
                        "bounds_mm": bbox_json(d.layer_bounds(Some(&layer)), &to_mm),
                    }),
                );
            }
            let blocks: Map<String, Value> = d
                .blocks
                .iter()
                .map(|b| {
                    let counts: BTreeMap<&str, usize> = count_entities(&b.entities)
                        .into_values()
                        .flatten()
                        .fold(BTreeMap::new(), |mut acc, (k, v)| {
                            *acc.entry(k).or_insert(0) += v;
                            acc
                        });
                    (b.name.clone(), json!({ "entities": counts }))
                })
                .collect();
            json!({
                "name": f.name,
                "file": f.file_name(),
                "format": "dxf",
                "units": unit_name(d.units),
                "description": f.description,
                "bounds_mm": bbox_json(d.bounds(), &to_mm),
                "layers": layers,
                "blocks": blocks,
                "expect": f.expect,
                "notes": f.notes,
            })
        })
        .collect();
    let gerber_entries: Vec<Value> = gerber::all()
        .iter()
        .map(|g| {
            json!({
                "name": g.name,
                "file": g.file_name(),
                "format": "gerber",
                "units": "mm",
                "description": g.description,
                "function": g.function,
                "expect": g.expect,
                "notes": g.notes,
            })
        })
        .collect();
    let entries: Vec<Value> = entries.into_iter().chain(gerber_entries).collect();
    json!({
        "generator": format!("lvp-fixtures {} (`just fixtures`)", env!("CARGO_PKG_VERSION")),
        "conventions": [
            "all *_mm values are millimetres regardless of the file's units",
            "DXF bounds are of centrelines: polyline constant widths are not included",
            "DXF entity counts are of entities as written, before any union or block resolution",
            "Gerber entries have one layer named after the file; `function` is the X2 TF.FileFunction",
        ],
        "fixtures": entries,
    })
}

/// Every file the generator produces, as `(relative path, contents)`.
pub fn render() -> Vec<(String, String)> {
    let fixtures = all();
    let mut files: Vec<(String, String)> = fixtures
        .iter()
        .map(|f| (f.file_name(), f.document.to_dxf()))
        .collect();
    files.extend(gerber::all().into_iter().map(|g| (g.file_name(), g.text)));
    let mut manifest =
        serde_json::to_string_pretty(&manifest(&fixtures)).expect("manifest serialises");
    manifest.push('\n');
    files.push((MANIFEST_FILE.to_string(), manifest));
    files
}

/// `fixtures/` directory at the repository root.
pub fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}
