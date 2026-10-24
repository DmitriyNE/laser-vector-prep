//! Synthetic GDSII fixture from a tiny stream emitter (independent of the core's reader):
//! a pad cell placed by SREFs with reflection, magnification and rotation, an AREF array,
//! paths with the three end styles, a BOX and a TEXT annotation.

use serde_json::{Value, json};

pub struct GdsFixture {
    pub name: &'static str,
    pub description: &'static str,
    pub bytes: Vec<u8>,
    pub expect: Value,
    pub notes: Vec<&'static str>,
}

impl GdsFixture {
    pub fn file_name(&self) -> String {
        format!("gds/{}.gds", self.name)
    }
}

mod rec {
    pub const HEADER: u8 = 0x00;
    pub const BGNLIB: u8 = 0x01;
    pub const LIBNAME: u8 = 0x02;
    pub const UNITS: u8 = 0x03;
    pub const ENDLIB: u8 = 0x04;
    pub const BGNSTR: u8 = 0x05;
    pub const STRNAME: u8 = 0x06;
    pub const ENDSTR: u8 = 0x07;
    pub const BOUNDARY: u8 = 0x08;
    pub const PATH: u8 = 0x09;
    pub const SREF: u8 = 0x0A;
    pub const AREF: u8 = 0x0B;
    pub const TEXT: u8 = 0x0C;
    pub const LAYER: u8 = 0x0D;
    pub const DATATYPE: u8 = 0x0E;
    pub const WIDTH: u8 = 0x0F;
    pub const XY: u8 = 0x10;
    pub const ENDEL: u8 = 0x11;
    pub const SNAME: u8 = 0x12;
    pub const COLROW: u8 = 0x13;
    pub const TEXTTYPE: u8 = 0x16;
    pub const STRING: u8 = 0x19;
    pub const STRANS: u8 = 0x1A;
    pub const MAG: u8 = 0x1B;
    pub const ANGLE: u8 = 0x1C;
    pub const PATHTYPE: u8 = 0x21;
    pub const BOX: u8 = 0x2D;
    pub const BOXTYPE: u8 = 0x2E;
    pub const BGNEXTN: u8 = 0x30;
    pub const ENDEXTN: u8 = 0x31;
}

struct Emitter {
    out: Vec<u8>,
}

/// GDSII 8-byte real (excess-64 exponent of 16, 56-bit fraction).
fn real8(v: f64) -> [u8; 8] {
    if v == 0.0 {
        return [0; 8];
    }
    let sign = if v < 0.0 { 0x80u8 } else { 0 };
    let mut m = v.abs();
    let mut e: i32 = 0;
    while m >= 1.0 {
        m /= 16.0;
        e += 1;
    }
    while m < 1.0 / 16.0 {
        m *= 16.0;
        e -= 1;
    }
    let mant = (m * 2f64.powi(56)).round() as u64;
    let mut out = [0u8; 8];
    out[0] = sign | ((e + 64) as u8 & 0x7f);
    for i in 0..7 {
        out[1 + i] = ((mant >> (8 * (6 - i))) & 0xff) as u8;
    }
    out
}

impl Emitter {
    fn record(&mut self, kind: u8, dtype: u8, data: &[u8]) {
        let len = (data.len() + 4) as u16;
        self.out.extend_from_slice(&len.to_be_bytes());
        self.out.push(kind);
        self.out.push(dtype);
        self.out.extend_from_slice(data);
    }
    fn i16(&mut self, kind: u8, v: i16) {
        self.record(kind, 2, &v.to_be_bytes());
    }
    fn i32(&mut self, kind: u8, v: i32) {
        self.record(kind, 3, &v.to_be_bytes());
    }
    fn name(&mut self, kind: u8, s: &str) {
        let mut d = s.as_bytes().to_vec();
        if d.len() % 2 == 1 {
            d.push(0);
        }
        self.record(kind, 6, &d);
    }
    fn xy(&mut self, pts: &[(i32, i32)]) {
        let mut d = Vec::new();
        for (x, y) in pts {
            d.extend_from_slice(&x.to_be_bytes());
            d.extend_from_slice(&y.to_be_bytes());
        }
        self.record(rec::XY, 3, &d);
    }
    fn begin_lib(&mut self, name: &str) {
        self.i16(rec::HEADER, 600);
        // Fixed timestamps keep the file byte-identical between runs.
        self.record(rec::BGNLIB, 2, &[0u8; 24]);
        self.name(rec::LIBNAME, name);
        let mut u = Vec::new();
        u.extend_from_slice(&real8(1e-3));
        u.extend_from_slice(&real8(1e-9));
        self.record(rec::UNITS, 5, &u);
    }
    fn begin_cell(&mut self, name: &str) {
        self.record(rec::BGNSTR, 2, &[0u8; 24]);
        self.name(rec::STRNAME, name);
    }
    fn end_cell(&mut self) {
        self.record(rec::ENDSTR, 0, &[]);
    }
    fn end_lib(&mut self) {
        self.record(rec::ENDLIB, 0, &[]);
    }
    fn boundary(&mut self, layer: i16, dtype: i16, pts: &[(i32, i32)]) {
        self.record(rec::BOUNDARY, 0, &[]);
        self.i16(rec::LAYER, layer);
        self.i16(rec::DATATYPE, dtype);
        let mut closed = pts.to_vec();
        closed.push(pts[0]);
        self.xy(&closed);
        self.record(rec::ENDEL, 0, &[]);
    }
    fn boxel(&mut self, layer: i16, boxtype: i16, pts: &[(i32, i32)]) {
        self.record(rec::BOX, 0, &[]);
        self.i16(rec::LAYER, layer);
        self.i16(rec::BOXTYPE, boxtype);
        let mut closed = pts.to_vec();
        closed.push(pts[0]);
        self.xy(&closed);
        self.record(rec::ENDEL, 0, &[]);
    }
    fn path(
        &mut self,
        layer: i16,
        width: i32,
        pathtype: i16,
        ext: Option<(i32, i32)>,
        pts: &[(i32, i32)],
    ) {
        self.record(rec::PATH, 0, &[]);
        self.i16(rec::LAYER, layer);
        self.i16(rec::DATATYPE, 0);
        self.i16(rec::PATHTYPE, pathtype);
        self.i32(rec::WIDTH, width);
        if let Some((b, e)) = ext {
            self.i32(rec::BGNEXTN, b);
            self.i32(rec::ENDEXTN, e);
        }
        self.xy(pts);
        self.record(rec::ENDEL, 0, &[]);
    }
    fn sref(&mut self, name: &str, reflect: bool, mag: f64, angle: f64, at: (i32, i32)) {
        self.record(rec::SREF, 0, &[]);
        self.name(rec::SNAME, name);
        if reflect || mag != 1.0 || angle != 0.0 {
            self.record(
                rec::STRANS,
                1,
                &(if reflect { 0x8000u16 } else { 0 }).to_be_bytes(),
            );
            if mag != 1.0 {
                self.record(rec::MAG, 5, &real8(mag));
            }
            if angle != 0.0 {
                self.record(rec::ANGLE, 5, &real8(angle));
            }
        }
        self.xy(&[at]);
        self.record(rec::ENDEL, 0, &[]);
    }
    fn aref(
        &mut self,
        name: &str,
        cols: i16,
        rows: i16,
        origin: (i32, i32),
        col_end: (i32, i32),
        row_end: (i32, i32),
    ) {
        self.record(rec::AREF, 0, &[]);
        self.name(rec::SNAME, name);
        let mut cr = Vec::new();
        cr.extend_from_slice(&cols.to_be_bytes());
        cr.extend_from_slice(&rows.to_be_bytes());
        self.record(rec::COLROW, 2, &cr);
        self.xy(&[origin, col_end, row_end]);
        self.record(rec::ENDEL, 0, &[]);
    }
    fn text(&mut self, layer: i16, at: (i32, i32), s: &str) {
        self.record(rec::TEXT, 0, &[]);
        self.i16(rec::LAYER, layer);
        self.i16(rec::TEXTTYPE, 0);
        self.xy(&[at]);
        self.name(rec::STRING, s);
        self.record(rec::ENDEL, 0, &[]);
    }
}

/// Database unit 1 nm, user unit 1 um; everything below is in nm.
fn cells() -> GdsFixture {
    let mut e = Emitter { out: Vec::new() };
    e.begin_lib("LVP");
    // PAD: a 20 um square on 1/0 with a 10 um square hole marked by a boundary on 2/0.
    e.begin_cell("PAD");
    e.boundary(1, 0, &[(0, 0), (20_000, 0), (20_000, 20_000), (0, 20_000)]);
    e.boundary(
        2,
        0,
        &[
            (5_000, 5_000),
            (15_000, 5_000),
            (15_000, 15_000),
            (5_000, 15_000),
        ],
    );
    e.end_cell();
    // TOP: pads placed plainly, mirrored + rotated + magnified, in a 3 x 2 array; paths with
    // the three end styles on 3/0; a BOX on 4/0; a TEXT on 5/0.
    e.begin_cell("TOP");
    e.sref("PAD", false, 1.0, 0.0, (0, 0));
    e.sref("PAD", true, 2.0, 90.0, (100_000, 0));
    e.aref("PAD", 3, 2, (0, 100_000), (90_000, 100_000), (0, 160_000));
    e.path(3, 4_000, 0, None, &[(200_000, 0), (260_000, 0)]); // flush 60 x 4
    e.path(3, 4_000, 1, None, &[(200_000, 20_000), (260_000, 20_000)]); // round ends
    e.path(3, 4_000, 2, None, &[(200_000, 40_000), (260_000, 40_000)]); // square ends: 64 x 4
    e.path(
        3,
        4_000,
        4,
        Some((1_000, 3_000)),
        &[(200_000, 60_000), (260_000, 60_000)],
    ); // 64 x 4
    e.path(
        3,
        4_000,
        0,
        None,
        &[(200_000, 80_000), (230_000, 80_000), (230_000, 110_000)],
    ); // mitred L
    e.boxel(
        4,
        0,
        &[
            (300_000, 0),
            (310_000, 0),
            (310_000, 30_000),
            (300_000, 30_000),
        ],
    );
    e.text(5, (0, 200_000), "LVP");
    e.end_cell();
    e.end_lib();

    // Areas in mm²: 1 um = 0.001 mm.
    let pad = 0.02f64 * 0.02; // 4e-4
    let hole = 0.01f64 * 0.01; // 1e-4
    let pads_plain = 1.0 + 6.0; // plain + array
    let pads_mag = 4.0; // 2x magnified
    GdsFixture {
        name: "cells",
        description: "PAD cell (20 um square on 1/0, 10 um square on 2/0) placed plainly, mirrored + 2x + 90 degrees, and as a 3 x 2 AREF; paths with flush, round, square, custom and mitred shapes on 3/0; a BOX on 4/0; TEXT on 5/0. 1 nm database unit, 1 um user unit.",
        bytes: e.out,
        expect: json!({
            "layers": ["1/0", "2/0", "3/0", "4/0"],
            "layer_1_islands": 8,
            "layer_1_area_mm2": pad * (pads_plain + pads_mag),
            "layer_2_area_mm2": hole * (pads_plain + pads_mag),
            "layer_3_fill_area_mm2": (0.06 * 0.004) + (0.064 * 0.004) * 2.0 + (0.032 * 0.004 + 0.004 * 0.028),
            "layer_3_round_paths": 1,
            "layer_3_round_width_mm": 0.004,
            "layer_4_area_mm2": 0.010 * 0.030,
            "bounds_mm": [0.0, -0.002, 0.31, 0.15],
            "diagnostics": ["gds.text"],
        }),
        notes: vec![
            "1/0 and 2/0 come from the same PAD cell: 8 placements each (1 + 6 in the array + 1 magnified)",
            "the mirrored, rotated, magnified placement maps the pad to x in [0.1, 0.14], y in [0, 0.04] mm",
            "flush/square/custom paths are exact polygons; the round-ended path stays a centreline with width",
            "the L-shaped flush path is mitred: a 32 x 4 leg plus a 4 x 28 leg (the mitre fills the outer corner)",
        ],
    }
}

pub fn all() -> Vec<GdsFixture> {
    vec![cells()]
}
