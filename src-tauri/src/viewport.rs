//! Polygon → triangulated vertex/index buffers for the WebGPU canvas
//! in the React frontend.
//!
//! H1.5 collapsed the original two-window setup (winit/wgpu native
//! window + Tauri webview) into a single window: rendering moved to
//! browser WebGPU inside a `<canvas>`. This module is what remains
//! of the Rust render path — just CPU-side prep of the buffers that
//! the JS uploads to GPU.

use serde::Serialize;

use crate::gds::{self, Polygon};

/// Deterministic colour per GDS layer. H2c will replace this with a
/// proper technology-file map.
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

/// Triangulate one polygon (no holes for now) into a flat vertex layout:
/// [x, y, r, g, b] per vertex.
fn tessellate(poly: &Polygon, verts: &mut Vec<f32>, indices: &mut Vec<u32>) {
    if poly.points.len() < 3 {
        return;
    }
    let mut flat: Vec<f64> = Vec::with_capacity(poly.points.len() * 2);
    for p in &poly.points {
        flat.push(p[0]);
        flat.push(p[1]);
    }
    let Ok(tris) = earcutr::earcut(&flat, &[], 2) else {
        log::warn!("earcut failed on polygon with {} pts", poly.points.len());
        return;
    };
    let base = (verts.len() / 5) as u32;
    let [r, g, b] = layer_color(poly.layer);
    for p in &poly.points {
        verts.push(p[0] as f32);
        verts.push(p[1] as f32);
        verts.push(r);
        verts.push(g);
        verts.push(b);
    }
    for i in tris {
        indices.push(base + i as u32);
    }
}

/// Buffers + metadata for the frontend renderer.
#[derive(Debug, Serialize)]
pub struct Scene {
    pub polygon_count: usize,
    pub layers: Vec<i16>,
    pub bbox_min: [f64; 2],
    pub bbox_max: [f64; 2],
    /// Interleaved float32: x, y, r, g, b per vertex.
    pub vertices: Vec<f32>,
    /// uint32 triangle indices.
    pub indices: Vec<u32>,
}

pub fn build_scene(polys: &[Polygon]) -> Scene {
    let bbox = gds::bbox(polys);
    let mut verts = Vec::new();
    let mut indices = Vec::new();
    for p in polys {
        tessellate(p, &mut verts, &mut indices);
    }
    let mut layers: Vec<i16> = polys.iter().map(|p| p.layer).collect();
    layers.sort_unstable();
    layers.dedup();
    Scene {
        polygon_count: polys.len(),
        layers,
        bbox_min: bbox.min,
        bbox_max: bbox.max,
        vertices: verts,
        indices,
    }
}
