//! Polygon → triangulated vertex/index buffers for the WebGPU canvas
//! in the React frontend.
//!
//! H2c: scene is grouped by layer so the frontend can toggle layer
//! visibility per draw call without re-uploading buffers. Each layer
//! also carries edge indices (line-list) so the frontend can render
//! a 1-pixel outline on top of the fill — keeps thin features visible
//! at any zoom (waveguides on a multi-mm die would otherwise drop to
//! sub-pixel and disappear).

use std::collections::BTreeMap;

use serde::Serialize;

use crate::gds::{self, LoadInfo, Polygon};
use crate::tech::Tech;

#[derive(Default)]
struct LayerBuf {
    vertices: Vec<f32>,
    triangle_indices: Vec<u32>,
    edge_indices: Vec<u32>,
    polygon_count: usize,
}

/// Per-(layer, datatype) GPU-ready buffers. Grouping by the pair (not
/// just the layer) matches how technology files distinguish styles —
/// e.g. layer 1 datatype 0 "active drawing" vs datatype 1 "active label".
#[derive(Debug, Serialize)]
pub struct LayerData {
    pub layer: i16,
    pub datatype: i16,
    /// Display name from the technology file (or a generated fallback).
    pub name: String,
    pub color: [f32; 3],
    pub polygon_count: usize,
    /// Interleaved float32: x, y, r, g, b per vertex.
    pub vertices: Vec<f32>,
    /// uint32 triangle indices (triangle-list).
    pub triangle_indices: Vec<u32>,
    /// uint32 edge indices (line-list, pairs around each polygon ring).
    pub edge_indices: Vec<u32>,
}

#[derive(Debug, Serialize)]
pub struct Scene {
    pub polygon_count: usize,
    pub bbox_min: [f64; 2],
    pub bbox_max: [f64; 2],
    pub layers: Vec<LayerData>,
    pub top_cell: String,
    pub cell_names: Vec<String>,
}

/// Result of a viewport click hit-test, IPC'd back to the React
/// inspector. `points` is the polygon ring (database units) so the
/// frontend can draw a highlight outline without re-deriving it.
#[derive(Debug, Serialize)]
pub struct PolygonHit {
    /// Index into the flattened polygon list (stable for the session).
    pub index: usize,
    pub layer: i16,
    pub datatype: i16,
    /// Style name from the technology file (or a generated fallback).
    pub name: String,
    pub point_count: usize,
    pub bbox_min: [f64; 2],
    pub bbox_max: [f64; 2],
    pub area: f64,
    /// Ring vertices for the highlight outline.
    pub points: Vec<[f64; 2]>,
}

/// Find the polygon under world point `(x, y)`. When several overlap,
/// the smallest-area one wins — that's almost always the most specific
/// feature the user meant to click (e.g. a contact inside a metal pad).
pub fn hit_test(polys: &[Polygon], x: f64, y: f64) -> Option<PolygonHit> {
    let p = [x, y];
    let mut best: Option<(usize, f64)> = None;
    for (i, poly) in polys.iter().enumerate() {
        if gds::point_in_polygon(&poly.points, p) {
            let area = gds::polygon_area(&poly.points);
            if best.map_or(true, |(_, a)| area < a) {
                best = Some((i, area));
            }
        }
    }
    let (index, area) = best?;
    let poly = &polys[index];
    let bb = gds::polygon_bbox(&poly.points);
    let style = Tech::default_tech().resolve(poly.layer, poly.datatype);
    Some(PolygonHit {
        index,
        layer: poly.layer,
        datatype: poly.datatype,
        name: style.name,
        point_count: poly.points.len(),
        bbox_min: bb.min,
        bbox_max: bb.max,
        area,
        points: poly.points.clone(),
    })
}

pub fn build_scene(info: &LoadInfo) -> Scene {
    let polys = &info.polygons;
    let tech = Tech::default_tech();
    let bbox = gds::bbox(polys);
    // Group by (layer, datatype) so the frontend can toggle / colour
    // each technology style independently. BTreeMap → stable sorted order.
    let mut by_style: BTreeMap<(i16, i16), LayerBuf> = BTreeMap::new();

    for p in polys {
        if p.points.len() < 3 {
            continue;
        }
        let mut flat: Vec<f64> = Vec::with_capacity(p.points.len() * 2);
        for pt in &p.points {
            flat.push(pt[0]);
            flat.push(pt[1]);
        }
        let Ok(tris) = earcutr::earcut(&flat, &[], 2) else {
            log::warn!("earcut failed on polygon with {} pts", p.points.len());
            continue;
        };

        let entry = by_style.entry((p.layer, p.datatype)).or_default();
        let base = (entry.vertices.len() / 5) as u32;
        let [r, g, b] = tech.color(p.layer, p.datatype);
        for pt in &p.points {
            entry.vertices.push(pt[0] as f32);
            entry.vertices.push(pt[1] as f32);
            entry.vertices.push(r);
            entry.vertices.push(g);
            entry.vertices.push(b);
        }
        for i in tris {
            entry.triangle_indices.push(base + i as u32);
        }
        // Edge ring: n pairs (i → (i+1) mod n).
        let n = p.points.len() as u32;
        for i in 0..n {
            entry.edge_indices.push(base + i);
            entry.edge_indices.push(base + (i + 1) % n);
        }
        entry.polygon_count += 1;
    }

    let polygon_count = by_style.values().map(|b| b.polygon_count).sum();
    let layers: Vec<LayerData> = by_style
        .into_iter()
        .map(|((layer, datatype), b)| {
            let style = tech.resolve(layer, datatype);
            LayerData {
                layer,
                datatype,
                name: style.name,
                color: style.color,
                polygon_count: b.polygon_count,
                vertices: b.vertices,
                triangle_indices: b.triangle_indices,
                edge_indices: b.edge_indices,
            }
        })
        .collect();

    Scene {
        polygon_count,
        bbox_min: bbox.min,
        bbox_max: bbox.max,
        layers,
        top_cell: info.top_cell.clone(),
        cell_names: info.cell_names.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(layer: i16, datatype: i16, x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon {
            layer,
            datatype,
            points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]],
        }
    }

    #[test]
    fn hit_test_picks_smallest_containing_polygon() {
        // A big metal pad (layer 4) with a small contact (layer 3) inside it.
        let polys = vec![
            rect(4, 0, 0.0, 0.0, 100.0, 100.0),
            rect(3, 7, 40.0, 40.0, 60.0, 60.0),
        ];

        // Click inside the contact → smallest-area wins (the contact).
        let hit = hit_test(&polys, 50.0, 50.0).expect("hit");
        assert_eq!(hit.index, 1);
        assert_eq!(hit.layer, 3);
        assert_eq!(hit.datatype, 7);
        assert_eq!(hit.point_count, 4);
        assert!((hit.area - 400.0).abs() < 1e-9);
        assert_eq!(hit.bbox_min, [40.0, 40.0]);
        assert_eq!(hit.bbox_max, [60.0, 60.0]);

        // Click in the pad but outside the contact → the pad.
        let hit = hit_test(&polys, 10.0, 10.0).expect("hit");
        assert_eq!(hit.index, 0);
        assert_eq!(hit.layer, 4);

        // Click outside everything → miss.
        assert!(hit_test(&polys, 200.0, 200.0).is_none());
    }
}
