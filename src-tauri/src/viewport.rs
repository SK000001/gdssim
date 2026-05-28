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

use crate::gds::{self, LoadInfo};

/// Deterministic colour per GDS layer. H2c-followup will swap this
/// for a JSON technology file (mapping (layer, datatype) → name + colour).
fn layer_color(layer: i16) -> [f32; 3] {
    const PALETTE: [[f32; 3]; 8] = [
        [0.55, 0.55, 0.55], // 0
        [0.45, 0.85, 0.40], // 1  poly
        [0.30, 0.55, 0.95], // 2  active
        [0.95, 0.85, 0.30], // 3  contact
        [0.90, 0.40, 0.40], // 4  metal1
        [0.40, 0.85, 0.85], // 5  metal2
        [0.85, 0.50, 0.85], // 6  metal3
        [0.95, 0.65, 0.30], // 7  via
    ];
    if (0..PALETTE.len() as i16).contains(&layer) {
        return PALETTE[layer as usize];
    }
    let h = (layer as i32 as u32).wrapping_mul(2654435761);
    let r = ((h >> 16) & 0xff) as f32 / 255.0;
    let g = ((h >> 8) & 0xff) as f32 / 255.0;
    let b = (h & 0xff) as f32 / 255.0;
    [0.4 + 0.5 * r, 0.4 + 0.5 * g, 0.4 + 0.5 * b]
}

#[derive(Default)]
struct LayerBuf {
    vertices: Vec<f32>,
    triangle_indices: Vec<u32>,
    edge_indices: Vec<u32>,
    polygon_count: usize,
}

/// Per-layer GPU-ready buffers.
#[derive(Debug, Serialize)]
pub struct LayerData {
    pub layer: i16,
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

pub fn build_scene(info: &LoadInfo) -> Scene {
    let polys = &info.polygons;
    let bbox = gds::bbox(polys);
    let mut by_layer: BTreeMap<i16, LayerBuf> = BTreeMap::new();

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

        let entry = by_layer.entry(p.layer).or_default();
        let base = (entry.vertices.len() / 5) as u32;
        let [r, g, b] = layer_color(p.layer);
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

    let polygon_count = by_layer.values().map(|b| b.polygon_count).sum();
    let layers: Vec<LayerData> = by_layer
        .into_iter()
        .map(|(layer, b)| LayerData {
            layer,
            color: layer_color(layer),
            polygon_count: b.polygon_count,
            vertices: b.vertices,
            triangle_indices: b.triangle_indices,
            edge_indices: b.edge_indices,
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
