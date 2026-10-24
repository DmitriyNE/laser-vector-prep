//! DXF export: one layer per exporting output group, LWPOLYLINEs with bulges so arcs stay
//! arcs, coordinates exactly as in the model (millimetres, no page offset). Layer colours
//! are the nearest AutoCAD colour index to the group's RGB; the true colour is written as
//! well where the writer supports it.

use cavalier_contours::polyline::PlineSource;

use crate::dxf::write::{Document, Entity, LwVertex};
use crate::model::Rgb;
use crate::pline::Pline;
use crate::units::Unit;

/// Nearest of the seven basic AutoCAD colour indices.
pub fn nearest_aci(c: Rgb) -> i16 {
    const TABLE: [(i16, [u8; 3]); 8] = [
        (1, [255, 0, 0]),
        (2, [255, 255, 0]),
        (3, [0, 255, 0]),
        (4, [0, 255, 255]),
        (5, [0, 0, 255]),
        (6, [255, 0, 255]),
        (7, [255, 255, 255]),
        (7, [0, 0, 0]),
    ];
    TABLE
        .iter()
        .min_by_key(|(_, rgb)| {
            let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
            d(rgb[0], c.0) + d(rgb[1], c.1) + d(rgb[2], c.2)
        })
        .map(|(aci, _)| *aci)
        .unwrap_or(7)
}

/// DXF layer names may not contain `<>/\":;?*|=\``; replace them.
pub fn layer_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if "<>/\\\":;?*|=`".contains(c) { '_' } else { c })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "GROUP".to_string()
    } else {
        trimmed.to_string()
    }
}

/// One exporting group's paths, exact (with arcs), in planned order.
pub struct DxfGroup {
    pub name: String,
    pub color: Rgb,
    pub paths: Vec<Pline>,
}

pub fn write(groups: &[DxfGroup]) -> String {
    let mut doc = Document::new(Unit::Millimetre);
    let mut used: Vec<String> = Vec::new();
    for g in groups {
        let mut name = layer_name(&g.name);
        let mut n = 2;
        while used.iter().any(|u| u.eq_ignore_ascii_case(&name)) {
            name = format!("{}_{n}", layer_name(&g.name));
            n += 1;
        }
        used.push(name.clone());
        doc.layer(&name, nearest_aci(g.color));
        for p in &g.paths {
            let count = p.vertex_count();
            if count < 2 {
                continue;
            }
            let vertices: Vec<LwVertex> = p
                .iter_vertexes()
                .enumerate()
                .map(|(i, v)| {
                    // The last vertex of an open polyline has no outgoing segment.
                    let bulge = if !p.is_closed() && i + 1 == count {
                        0.0
                    } else {
                        v.bulge
                    };
                    LwVertex::bulged(v.x, v.y, bulge)
                })
                .collect();
            doc.add(
                &name,
                Entity::LwPolyline {
                    vertices,
                    closed: p.is_closed(),
                    const_width: None,
                },
            );
        }
    }
    doc.to_dxf()
}
