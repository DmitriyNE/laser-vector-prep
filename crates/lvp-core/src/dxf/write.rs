//! Minimal DXF R2000 (AC1015) writer: LINE, CIRCLE, ARC, LWPOLYLINE (bulges, constant
//! width), INSERT with blocks, named layers and `$INSUNITS`.
//!
//! Output is deterministic: sequential handle allocation and fixed float formatting, so
//! regenerated fixtures diff cleanly. The writer also knows how to compute the bounds of
//! what it writes (centrelines only; polyline widths are ignored), which the fixture
//! manifest relies on.

use std::fmt::Write as _;

use crate::arc::Arc;
use crate::geom::{BBox, Point};
use crate::units::Unit;

/// Tolerance used when flattening curves for bounds computation.
const BOUNDS_TOLERANCE: f64 = 1e-6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LwVertex {
    pub x: f64,
    pub y: f64,
    /// `tan(sweep / 4)` of the arc from this vertex to the next; 0 for a straight segment.
    pub bulge: f64,
}

impl LwVertex {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y, bulge: 0.0 }
    }

    pub const fn bulged(x: f64, y: f64, bulge: f64) -> Self {
        Self { x, y, bulge }
    }

    fn point(&self) -> Point {
        Point::new(self.x, self.y)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entity {
    Line {
        start: Point,
        end: Point,
    },
    Circle {
        center: Point,
        radius: f64,
    },
    /// Degrees, counter-clockwise from `start_deg` to `end_deg` (DXF convention).
    Arc {
        center: Point,
        radius: f64,
        start_deg: f64,
        end_deg: f64,
    },
    LwPolyline {
        vertices: Vec<LwVertex>,
        closed: bool,
        const_width: Option<f64>,
    },
    /// Block reference: uniform `scale`, then rotation, then translation to `at`.
    Insert {
        block: String,
        at: Point,
        scale: f64,
        rotation_deg: f64,
    },
}

impl Entity {
    pub fn type_name(&self) -> &'static str {
        match self {
            Entity::Line { .. } => "LINE",
            Entity::Circle { .. } => "CIRCLE",
            Entity::Arc { .. } => "ARC",
            Entity::LwPolyline { .. } => "LWPOLYLINE",
            Entity::Insert { .. } => "INSERT",
        }
    }

    pub fn rect(min: Point, max: Point) -> Entity {
        Entity::LwPolyline {
            vertices: vec![
                LwVertex::new(min.x, min.y),
                LwVertex::new(max.x, min.y),
                LwVertex::new(max.x, max.y),
                LwVertex::new(min.x, max.y),
            ],
            closed: true,
            const_width: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub layer: String,
    pub entity: Entity,
}

/// Layer with an AutoCAD Color Index (1 red, 2 yellow, 3 green, 4 cyan, 5 blue, 6 magenta, 7 white).
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub name: String,
    pub color: i16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub name: String,
    pub entities: Vec<Placed>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    pub units: Unit,
    pub layers: Vec<Layer>,
    pub blocks: Vec<Block>,
    pub entities: Vec<Placed>,
}

/// Similarity transform used to resolve block references: `p' = R(scale * p) + translation`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Transform {
    scale: f64,
    rotation: f64,
    translation: Point,
}

impl Transform {
    const IDENTITY: Transform = Transform {
        scale: 1.0,
        rotation: 0.0,
        translation: Point::new(0.0, 0.0),
    };

    fn apply(&self, p: Point) -> Point {
        let (s, c) = self.rotation.sin_cos();
        let (x, y) = (p.x * self.scale, p.y * self.scale);
        Point::new(
            c * x - s * y + self.translation.x,
            s * x + c * y + self.translation.y,
        )
    }

    /// `self` applied after `inner` (inner is the block-local transform).
    fn compose(&self, inner: Transform) -> Transform {
        Transform {
            scale: self.scale * inner.scale,
            rotation: self.rotation + inner.rotation,
            translation: self.apply(inner.translation),
        }
    }
}

impl Document {
    pub fn new(units: Unit) -> Self {
        Self {
            units,
            layers: Vec::new(),
            blocks: Vec::new(),
            entities: Vec::new(),
        }
    }

    pub fn layer(&mut self, name: &str, color: i16) -> &mut Self {
        self.layers.push(Layer {
            name: name.to_string(),
            color,
        });
        self
    }

    pub fn add(&mut self, layer: &str, entity: Entity) -> &mut Self {
        self.entities.push(Placed {
            layer: layer.to_string(),
            entity,
        });
        self
    }

    pub fn block(&mut self, name: &str, entities: Vec<Placed>) -> &mut Self {
        self.blocks.push(Block {
            name: name.to_string(),
            entities,
        });
        self
    }

    fn find_block(&self, name: &str) -> Option<&Block> {
        self.blocks.iter().find(|b| b.name == name)
    }

    /// Extreme points of an entity after `t`, appended to `out`. Curves are flattened to
    /// `BOUNDS_TOLERANCE`; circles are handled exactly.
    fn extreme_points(&self, entity: &Entity, t: Transform, out: &mut Vec<Point>) {
        match entity {
            Entity::Line { start, end } => out.extend([t.apply(*start), t.apply(*end)]),
            Entity::Circle { center, radius } => {
                let c = t.apply(*center);
                let r = radius * t.scale;
                out.extend([
                    Point::new(c.x - r, c.y),
                    Point::new(c.x + r, c.y),
                    Point::new(c.x, c.y - r),
                    Point::new(c.x, c.y + r),
                ]);
            }
            Entity::Arc {
                center,
                radius,
                start_deg,
                end_deg,
            } => {
                let arc = Arc::from_dxf_angles(*center, *radius, *start_deg, *end_deg);
                if let Ok(pts) = arc.flatten(BOUNDS_TOLERANCE / t.scale.max(1e-12)) {
                    out.extend(pts.into_iter().map(|p| t.apply(p)));
                }
            }
            Entity::LwPolyline {
                vertices, closed, ..
            } => {
                let n = vertices.len();
                for (i, v) in vertices.iter().enumerate() {
                    out.push(t.apply(v.point()));
                    let next = if i + 1 < n {
                        Some(vertices[i + 1])
                    } else if *closed {
                        vertices.first().copied()
                    } else {
                        None
                    };
                    let arc = next.and_then(|nx| Arc::from_bulge(v.point(), nx.point(), v.bulge));
                    if let Some(pts) =
                        arc.and_then(|a| a.flatten(BOUNDS_TOLERANCE / t.scale.max(1e-12)).ok())
                    {
                        out.extend(pts.into_iter().map(|p| t.apply(p)));
                    }
                }
            }
            Entity::Insert {
                block,
                at,
                scale,
                rotation_deg,
            } => {
                if let Some(b) = self.find_block(block) {
                    let local = Transform {
                        scale: *scale,
                        rotation: rotation_deg.to_radians(),
                        translation: *at,
                    };
                    let tt = t.compose(local);
                    for p in &b.entities {
                        self.extreme_points(&p.entity, tt, out);
                    }
                }
            }
        }
    }

    /// Bounds of all model-space entities, in document units. Block references are resolved.
    pub fn bounds(&self) -> Option<BBox> {
        self.layer_bounds(None)
    }

    /// Bounds of the entities on one layer (`Some(name)`) or all (`None`), in document units.
    pub fn layer_bounds(&self, layer: Option<&str>) -> Option<BBox> {
        let mut pts = Vec::new();
        for p in &self.entities {
            if layer.is_none_or(|l| l == p.layer) {
                self.extreme_points(&p.entity, Transform::IDENTITY, &mut pts);
            }
        }
        BBox::of_points(pts)
    }

    /// Serialise to DXF text.
    pub fn to_dxf(&self) -> String {
        Writer::new(self).write()
    }
}

/// Fixed float formatting: six decimals, trailing zeros trimmed, never `-0.0`.
pub fn fmt_num(v: f64) -> String {
    let mut s = format!("{v:.6}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.push('0');
        }
    }
    if s == "-0.0" { "0.0".to_string() } else { s }
}

struct Writer<'a> {
    doc: &'a Document,
    out: String,
    next_handle: u32,
}

// Fixed handles for the skeleton objects; entities start after these.
const H_LTYPE_TABLE: u32 = 0x05;
const H_LTYPE_CONTINUOUS: u32 = 0x06;
const H_LAYER_TABLE: u32 = 0x02;
const H_STYLE_TABLE: u32 = 0x03;
const H_BLOCKREC_TABLE: u32 = 0x01;
const H_MODEL_SPACE: u32 = 0x1F;
const H_PAPER_SPACE: u32 = 0x1B;
const H_ROOT_DICT: u32 = 0x0C;
const H_GROUP_DICT: u32 = 0x0D;
const H_FIRST_FREE: u32 = 0x30;

impl<'a> Writer<'a> {
    fn new(doc: &'a Document) -> Self {
        Self {
            doc,
            out: String::new(),
            next_handle: H_FIRST_FREE,
        }
    }

    fn handle(&mut self) -> u32 {
        let h = self.next_handle;
        self.next_handle += 1;
        h
    }

    fn pair(&mut self, code: u16, value: impl AsRef<str>) {
        let _ = writeln!(self.out, "{code:>3}\n{}", value.as_ref());
    }

    fn num(&mut self, code: u16, v: f64) {
        let s = fmt_num(v);
        self.pair(code, s);
    }

    fn hex(&mut self, code: u16, h: u32) {
        self.pair(code, format!("{h:X}"));
    }

    fn write(mut self) -> String {
        // Body first so the final handle seed is known for the header.
        let body = {
            let mut b = Writer {
                doc: self.doc,
                out: String::new(),
                next_handle: self.next_handle,
            };
            b.tables();
            b.blocks();
            b.entities();
            b.objects();
            self.next_handle = b.next_handle;
            b.out
        };
        self.header();
        self.out.push_str(&body);
        self.pair(0, "EOF");
        self.out
    }

    fn header(&mut self) {
        self.pair(0, "SECTION");
        self.pair(2, "HEADER");
        self.pair(9, "$ACADVER");
        self.pair(1, "AC1015");
        self.pair(9, "$HANDSEED");
        let seed = self.next_handle;
        self.hex(5, seed);
        self.pair(9, "$INSUNITS");
        self.pair(70, self.doc.units.dxf_insunits().to_string());
        self.pair(9, "$MEASUREMENT");
        self.pair(
            70,
            if self.doc.units == Unit::Inch {
                "0"
            } else {
                "1"
            },
        );
        self.pair(0, "ENDSEC");
        self.pair(0, "SECTION");
        self.pair(2, "CLASSES");
        self.pair(0, "ENDSEC");
    }

    fn table_begin(&mut self, name: &str, handle: u32, count: usize) {
        self.pair(0, "TABLE");
        self.pair(2, name);
        self.hex(5, handle);
        self.pair(330, "0");
        self.pair(100, "AcDbSymbolTable");
        self.pair(70, count.to_string());
    }

    fn tables(&mut self) {
        self.pair(0, "SECTION");
        self.pair(2, "TABLES");

        self.table_begin("LTYPE", H_LTYPE_TABLE, 1);
        self.pair(0, "LTYPE");
        self.hex(5, H_LTYPE_CONTINUOUS);
        self.hex(330, H_LTYPE_TABLE);
        self.pair(100, "AcDbSymbolTableRecord");
        self.pair(100, "AcDbLinetypeTableRecord");
        self.pair(2, "CONTINUOUS");
        self.pair(70, "0");
        self.pair(3, "Solid line");
        self.pair(72, "65");
        self.pair(73, "0");
        self.num(40, 0.0);
        self.pair(0, "ENDTAB");

        let layers = self.doc.layers.clone();
        self.table_begin("LAYER", H_LAYER_TABLE, layers.len() + 1);
        for layer in std::iter::once(Layer {
            name: "0".to_string(),
            color: 7,
        })
        .chain(layers)
        {
            let h = self.handle();
            self.pair(0, "LAYER");
            self.hex(5, h);
            self.hex(330, H_LAYER_TABLE);
            self.pair(100, "AcDbSymbolTableRecord");
            self.pair(100, "AcDbLayerTableRecord");
            self.pair(2, &layer.name);
            self.pair(70, "0");
            self.pair(62, layer.color.to_string());
            self.pair(6, "CONTINUOUS");
            self.pair(390, "F");
        }
        self.pair(0, "ENDTAB");

        self.table_begin("STYLE", H_STYLE_TABLE, 0);
        self.pair(0, "ENDTAB");

        let block_names: Vec<String> = self.doc.blocks.iter().map(|b| b.name.clone()).collect();
        self.table_begin("BLOCK_RECORD", H_BLOCKREC_TABLE, block_names.len() + 2);
        for (name, h) in [
            ("*Model_Space", H_MODEL_SPACE),
            ("*Paper_Space", H_PAPER_SPACE),
        ] {
            self.block_record(name, h);
        }
        for name in &block_names {
            let h = self.block_record_handle(name);
            self.block_record(name, h);
        }
        self.pair(0, "ENDTAB");

        self.pair(0, "ENDSEC");
    }

    /// Block record handles are deterministic: allocated from a fixed range by index.
    fn block_record_handle(&self, name: &str) -> u32 {
        let idx = self
            .doc
            .blocks
            .iter()
            .position(|b| b.name == name)
            .expect("block exists");
        0x100 + idx as u32
    }

    fn block_record(&mut self, name: &str, handle: u32) {
        self.pair(0, "BLOCK_RECORD");
        self.hex(5, handle);
        self.hex(330, H_BLOCKREC_TABLE);
        self.pair(100, "AcDbSymbolTableRecord");
        self.pair(100, "AcDbBlockTableRecord");
        self.pair(2, name);
    }

    fn blocks(&mut self) {
        self.pair(0, "SECTION");
        self.pair(2, "BLOCKS");
        for (name, h) in [
            ("*Model_Space", H_MODEL_SPACE),
            ("*Paper_Space", H_PAPER_SPACE),
        ] {
            self.block_begin(name, h);
            self.block_end(h);
        }
        let blocks = self.doc.blocks.clone();
        for block in &blocks {
            let h = self.block_record_handle(&block.name);
            self.block_begin(&block.name, h);
            for p in &block.entities {
                self.entity(p, h);
            }
            self.block_end(h);
        }
        self.pair(0, "ENDSEC");
    }

    fn block_begin(&mut self, name: &str, owner: u32) {
        let h = self.handle();
        self.pair(0, "BLOCK");
        self.hex(5, h);
        self.hex(330, owner);
        self.pair(100, "AcDbEntity");
        self.pair(8, "0");
        self.pair(100, "AcDbBlockBegin");
        self.pair(2, name);
        self.pair(70, "0");
        self.num(10, 0.0);
        self.num(20, 0.0);
        self.num(30, 0.0);
        self.pair(3, name);
        self.pair(1, "");
    }

    fn block_end(&mut self, owner: u32) {
        let h = self.handle();
        self.pair(0, "ENDBLK");
        self.hex(5, h);
        self.hex(330, owner);
        self.pair(100, "AcDbEntity");
        self.pair(8, "0");
        self.pair(100, "AcDbBlockEnd");
    }

    fn entities(&mut self) {
        self.pair(0, "SECTION");
        self.pair(2, "ENTITIES");
        for p in &self.doc.entities.clone() {
            self.entity(p, H_MODEL_SPACE);
        }
        self.pair(0, "ENDSEC");
    }

    fn entity_begin(&mut self, kind: &str, layer: &str, owner: u32) {
        let h = self.handle();
        self.pair(0, kind);
        self.hex(5, h);
        self.hex(330, owner);
        self.pair(100, "AcDbEntity");
        self.pair(8, layer);
    }

    fn xy(&mut self, base: u16, p: Point) {
        self.num(base, p.x);
        self.num(base + 10, p.y);
        self.num(base + 20, 0.0);
    }

    fn entity(&mut self, placed: &Placed, owner: u32) {
        let layer = placed.layer.as_str();
        match &placed.entity {
            Entity::Line { start, end } => {
                self.entity_begin("LINE", layer, owner);
                self.pair(100, "AcDbLine");
                self.xy(10, *start);
                self.xy(11, *end);
            }
            Entity::Circle { center, radius } => {
                self.entity_begin("CIRCLE", layer, owner);
                self.pair(100, "AcDbCircle");
                self.xy(10, *center);
                self.num(40, *radius);
            }
            Entity::Arc {
                center,
                radius,
                start_deg,
                end_deg,
            } => {
                self.entity_begin("ARC", layer, owner);
                self.pair(100, "AcDbCircle");
                self.xy(10, *center);
                self.num(40, *radius);
                self.pair(100, "AcDbArc");
                self.num(50, *start_deg);
                self.num(51, *end_deg);
            }
            Entity::LwPolyline {
                vertices,
                closed,
                const_width,
            } => {
                self.entity_begin("LWPOLYLINE", layer, owner);
                self.pair(100, "AcDbPolyline");
                self.pair(90, vertices.len().to_string());
                self.pair(70, if *closed { "1" } else { "0" });
                if let Some(w) = const_width {
                    self.num(43, *w);
                }
                for v in vertices {
                    self.num(10, v.x);
                    self.num(20, v.y);
                    if v.bulge != 0.0 {
                        self.num(42, v.bulge);
                    }
                }
            }
            Entity::Insert {
                block,
                at,
                scale,
                rotation_deg,
            } => {
                self.entity_begin("INSERT", layer, owner);
                self.pair(100, "AcDbBlockReference");
                self.pair(2, block);
                self.xy(10, *at);
                self.num(41, *scale);
                self.num(42, *scale);
                self.num(43, *scale);
                self.num(50, *rotation_deg);
            }
        }
    }

    fn objects(&mut self) {
        self.pair(0, "SECTION");
        self.pair(2, "OBJECTS");
        self.pair(0, "DICTIONARY");
        self.hex(5, H_ROOT_DICT);
        self.pair(330, "0");
        self.pair(100, "AcDbDictionary");
        self.pair(281, "1");
        self.pair(3, "ACAD_GROUP");
        self.hex(350, H_GROUP_DICT);
        self.pair(0, "DICTIONARY");
        self.hex(5, H_GROUP_DICT);
        self.hex(330, H_ROOT_DICT);
        self.pair(100, "AcDbDictionary");
        self.pair(281, "1");
        self.pair(0, "ENDSEC");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> Document {
        let mut d = Document::new(Unit::Millimetre);
        d.layer("A", 5).layer("B", 1);
        d.add(
            "A",
            Entity::Line {
                start: Point::new(0.0, 0.0),
                end: Point::new(10.0, 5.0),
            },
        );
        d.add(
            "B",
            Entity::Circle {
                center: Point::new(20.0, 0.0),
                radius: 2.0,
            },
        );
        d.block(
            "PAD",
            vec![Placed {
                layer: "0".into(),
                entity: Entity::rect(Point::new(-1.0, -1.0), Point::new(1.0, 1.0)),
            }],
        );
        d.add(
            "A",
            Entity::Insert {
                block: "PAD".into(),
                at: Point::new(0.0, 20.0),
                scale: 2.0,
                rotation_deg: 45.0,
            },
        );
        d
    }

    #[test]
    fn output_is_deterministic_and_well_formed() {
        let a = sample().to_dxf();
        let b = sample().to_dxf();
        assert_eq!(a, b);
        assert!(a.starts_with("  0\nSECTION\n  2\nHEADER\n"));
        assert!(a.ends_with("  0\nEOF\n"));
        assert_eq!(a.matches("\nLWPOLYLINE\n").count(), 1);
        assert!(a.contains("$INSUNITS\n 70\n4\n"));
    }

    #[test]
    fn bounds_resolve_blocks_with_scale_and_rotation() {
        let b = sample().bounds().unwrap();
        // Rotated, scaled square: half-diagonal 2*sqrt(2) around (0, 20); circle to x = 22.
        let d = 2.0 * 2f64.sqrt();
        assert!((b.min.x + d).abs() < 1e-6);
        assert!((b.max.y - (20.0 + d)).abs() < 1e-6);
        assert!((b.max.x - 22.0).abs() < 1e-9);
        assert!((b.min.y + 2.0).abs() < 1e-9);
    }

    #[test]
    fn layer_bounds_filter() {
        let b = sample().layer_bounds(Some("B")).unwrap();
        assert_eq!(
            b,
            BBox {
                min: Point::new(18.0, -2.0),
                max: Point::new(22.0, 2.0)
            }
        );
        assert_eq!(sample().layer_bounds(Some("nope")), None);
    }

    #[test]
    fn number_formatting() {
        assert_eq!(fmt_num(25.0), "25.0");
        assert_eq!(fmt_num(0.0254), "0.0254");
        assert_eq!(fmt_num(-0.0), "0.0");
        assert_eq!(fmt_num(1.0 / 3.0), "0.333333");
    }

    proptest! {
        #[test]
        fn fmt_num_roundtrips_to_a_micron_thousandth(v in -1e5..1e5f64) {
            let back: f64 = fmt_num(v).parse().unwrap();
            prop_assert!((back - v).abs() <= 5e-7);
        }

        #[test]
        fn polyline_bounds_contain_vertices_and_arcs(
            vs in prop::collection::vec((-50.0..50.0f64, -50.0..50.0f64, -1.0..1.0f64), 2..8),
            closed in any::<bool>(),
        ) {
            let vertices: Vec<LwVertex> = vs.iter().map(|(x, y, b)| LwVertex::bulged(*x, *y, *b)).collect();
            let mut d = Document::new(Unit::Millimetre);
            d.add("L", Entity::LwPolyline { vertices: vertices.clone(), closed, const_width: None });
            let b = d.bounds().unwrap();
            for v in &vertices {
                prop_assert!(b.contains(v.point()));
            }
            // A bulged segment's arc midpoint must also be inside.
            for w in vertices.windows(2) {
                if let Some(arc) = Arc::from_bulge(w[0].point(), w[1].point(), w[0].bulge) {
                    let m = arc.point_at(0.5);
                    let grown = b.expanded(1e-6).unwrap();
                    prop_assert!(grown.contains(m));
                }
            }
        }
    }
}
