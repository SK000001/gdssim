//! GDS-II loading + hierarchy flattening.
//!
//! Wraps the `gds21` crate. Output of [`load_and_flatten`] is a flat
//! list of [`Polygon`] in database (integer) units — the viewport
//! triangulates them and converts to f32 for wgpu.
//!
//! Hierarchy: structure references (`SREF`) and array references
//! (`AREF`) are expanded with their `STRANS` transform (reflect about
//! X → magnify → rotate → translate). The top cell is auto-detected
//! as the first structure that no other structure references; if
//! every structure is referenced (or the file has only one), the
//! last-defined structure is used.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gds21::{GdsElement, GdsLibrary, GdsPoint, GdsStruct};
use thiserror::Error;

/// A flattened polygon in GDS database units (integer).
#[derive(Debug, Clone)]
pub struct Polygon {
    pub layer: i16,
    #[allow(dead_code)] // surfaced via inspector in H2d
    pub datatype: i16,
    /// Closed ring; first point is NOT repeated as the last.
    pub points: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, Copy)]
pub struct Bbox {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

impl Bbox {
    fn empty() -> Self {
        Self {
            min: [f64::INFINITY, f64::INFINITY],
            max: [f64::NEG_INFINITY, f64::NEG_INFINITY],
        }
    }
    fn expand(&mut self, p: [f64; 2]) {
        if p[0] < self.min[0] { self.min[0] = p[0]; }
        if p[1] < self.min[1] { self.min[1] = p[1]; }
        if p[0] > self.max[0] { self.max[0] = p[0]; }
        if p[1] > self.max[1] { self.max[1] = p[1]; }
    }
}

#[derive(Debug, Error)]
pub enum GdsError {
    #[error("gds21 parse error: {0}")]
    Parse(String),
    #[error("library has no structures")]
    Empty,
}

/// Load `path` and return all polygons of the top cell, with the
/// hierarchy flattened.
pub fn load_and_flatten(path: &Path) -> Result<Vec<Polygon>, GdsError> {
    let lib = GdsLibrary::load(path).map_err(|e| GdsError::Parse(format!("{e:?}")))?;
    flatten_library(&lib)
}

/// Like [`load_and_flatten`] but starts from a pre-parsed library.
pub fn flatten_library(lib: &GdsLibrary) -> Result<Vec<Polygon>, GdsError> {
    if lib.structs.is_empty() {
        return Err(GdsError::Empty);
    }
    let by_name: HashMap<&str, &GdsStruct> =
        lib.structs.iter().map(|s| (s.name.as_str(), s)).collect();
    let top = top_struct(lib);
    let mut out = Vec::new();
    flatten_struct(top, IDENTITY, &by_name, &mut out, 0);
    Ok(out)
}

/// Bbox of a polygon set in database units.
pub fn bbox(polys: &[Polygon]) -> Bbox {
    let mut b = Bbox::empty();
    for p in polys {
        for pt in &p.points {
            b.expand(*pt);
        }
    }
    b
}

/// Pick the "top" structure: one that no other struct references.
/// Falls back to the last-defined struct if all are referenced (or
/// only one exists).
fn top_struct(lib: &GdsLibrary) -> &GdsStruct {
    let mut referenced: HashSet<&str> = HashSet::new();
    for s in &lib.structs {
        for el in &s.elems {
            match el {
                GdsElement::GdsStructRef(r) => { referenced.insert(r.name.as_str()); }
                GdsElement::GdsArrayRef(r)  => { referenced.insert(r.name.as_str()); }
                _ => {}
            }
        }
    }
    lib.structs
        .iter()
        .find(|s| !referenced.contains(s.name.as_str()))
        .unwrap_or_else(|| lib.structs.last().expect("non-empty checked by caller"))
}

/// 2D affine: x' = a*x + b*y + tx, y' = c*x + d*y + ty.
#[derive(Debug, Clone, Copy)]
struct Affine { a: f64, b: f64, c: f64, d: f64, tx: f64, ty: f64 }

const IDENTITY: Affine = Affine { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

impl Affine {
    fn apply(&self, p: [f64; 2]) -> [f64; 2] {
        [self.a * p[0] + self.b * p[1] + self.tx,
         self.c * p[0] + self.d * p[1] + self.ty]
    }
    fn then(&self, outer: Affine) -> Affine {
        // self applied first, then outer.
        Affine {
            a:  outer.a * self.a + outer.b * self.c,
            b:  outer.a * self.b + outer.b * self.d,
            c:  outer.c * self.a + outer.d * self.c,
            d:  outer.c * self.b + outer.d * self.d,
            tx: outer.a * self.tx + outer.b * self.ty + outer.tx,
            ty: outer.c * self.tx + outer.d * self.ty + outer.ty,
        }
    }
}

/// Build the local transform of a SREF/AREF cell-instance.
/// Order per GDS spec: reflect about X (negate Y) → magnify → rotate → translate.
fn ref_transform(
    origin: GdsPoint,
    strans: Option<&gds21::GdsStrans>,
) -> Affine {
    let (refl, mag, ang_deg) = match strans {
        Some(s) => (s.reflected, s.mag.unwrap_or(1.0), s.angle.unwrap_or(0.0)),
        None => (false, 1.0, 0.0),
    };
    let theta = ang_deg.to_radians();
    let (ct, st) = (theta.cos(), theta.sin());
    let sy = if refl { -1.0 } else { 1.0 };
    // M = T * R * M_scale * M_refl
    Affine {
        a:  mag * ct,            // x ← x: cos * mag
        b: -mag * sy * st,       // x ← y: -sin * mag * sy
        c:  mag * st,             // y ← x:  sin * mag
        d:  mag * sy * ct,        // y ← y:  cos * mag * sy
        tx: origin.x as f64,
        ty: origin.y as f64,
    }
}

fn flatten_struct(
    s: &GdsStruct,
    parent: Affine,
    by_name: &HashMap<&str, &GdsStruct>,
    out: &mut Vec<Polygon>,
    depth: u32,
) {
    if depth > 64 {
        log::warn!("flatten: depth limit hit on '{}', skipping deeper refs", s.name);
        return;
    }
    for el in &s.elems {
        match el {
            GdsElement::GdsBoundary(b) => {
                let mut pts: Vec<[f64; 2]> = b.xy.iter().map(|p| parent.apply([p.x as f64, p.y as f64])).collect();
                // GDS boundaries close the ring (last == first); drop the duplicate.
                if pts.len() >= 2 && pts.first() == pts.last() {
                    pts.pop();
                }
                if pts.len() >= 3 {
                    out.push(Polygon { layer: b.layer, datatype: b.datatype, points: pts });
                }
            }
            GdsElement::GdsBox(bx) => {
                let pts = bx.xy.iter().map(|p| parent.apply([p.x as f64, p.y as f64])).collect::<Vec<_>>();
                if pts.len() >= 3 {
                    out.push(Polygon { layer: bx.layer, datatype: bx.boxtype, points: pts });
                }
            }
            GdsElement::GdsStructRef(r) => {
                let child = match by_name.get(r.name.as_str()) {
                    Some(c) => *c,
                    None => { log::warn!("missing struct {}", r.name); continue; }
                };
                let local = ref_transform(r.xy.clone(), r.strans.as_ref());
                let combined = local.then(parent);
                flatten_struct(child, combined, by_name, out, depth + 1);
            }
            GdsElement::GdsArrayRef(r) => {
                let child = match by_name.get(r.name.as_str()) {
                    Some(c) => *c,
                    None => { log::warn!("missing struct {}", r.name); continue; }
                };
                // r.xy: [origin, row_end (after cols steps), col_end (after rows steps)]
                if r.xy.len() < 3 { continue; }
                let o = [r.xy[0].x as f64, r.xy[0].y as f64];
                let re = [r.xy[1].x as f64, r.xy[1].y as f64];
                let ce = [r.xy[2].x as f64, r.xy[2].y as f64];
                let cols = r.cols.max(1) as f64;
                let rows = r.rows.max(1) as f64;
                let dcol = [(re[0] - o[0]) / cols, (re[1] - o[1]) / cols];
                let drow = [(ce[0] - o[0]) / rows, (ce[1] - o[1]) / rows];
                for ri in 0..r.rows {
                    for ci in 0..r.cols {
                        let origin_off = GdsPoint::new(
                            (o[0] + dcol[0] * ci as f64 + drow[0] * ri as f64) as i32,
                            (o[1] + dcol[1] * ci as f64 + drow[1] * ri as f64) as i32,
                        );
                        let local = ref_transform(origin_off, r.strans.as_ref());
                        let combined = local.then(parent);
                        flatten_struct(child, combined, by_name, out, depth + 1);
                    }
                }
            }
            // Paths/text/nodes ignored for H2a — H2b+ may add path-to-polygon.
            GdsElement::GdsPath(_)
            | GdsElement::GdsTextElem(_)
            | GdsElement::GdsNode(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gds21::{GdsBoundary, GdsLibrary, GdsPoint, GdsStruct, GdsStructRef, GdsUnits};
    use tempfile::tempdir;

    fn rect(layer: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> GdsBoundary {
        GdsBoundary {
            layer,
            datatype: 0,
            xy: vec![
                GdsPoint::new(x0, y0),
                GdsPoint::new(x1, y0),
                GdsPoint::new(x1, y1),
                GdsPoint::new(x0, y1),
                GdsPoint::new(x0, y0),
            ],
            ..Default::default()
        }
    }

    fn make_sample_library() -> GdsLibrary {
        // Child cell: one rectangle on layer 1.
        let child = GdsStruct {
            name: "CHILD".into(),
            elems: vec![GdsElement::GdsBoundary(rect(1, 0, 0, 1000, 500))],
            ..Default::default()
        };
        // Top cell: rectangle on layer 2 + one SREF to CHILD translated by (2000, 0).
        let top = GdsStruct {
            name: "TOP".into(),
            elems: vec![
                GdsElement::GdsBoundary(rect(2, 0, 1000, 500, 1500)),
                GdsElement::GdsStructRef(GdsStructRef {
                    name: "CHILD".into(),
                    xy: GdsPoint::new(2000, 0),
                    strans: None,
                    ..Default::default()
                }),
            ],
            ..Default::default()
        };
        GdsLibrary {
            name: "SAMPLE".into(),
            units: GdsUnits::new(0.001, 1e-9),
            structs: vec![child, top],
            ..Default::default()
        }
    }

    #[test]
    fn round_trips_and_flattens() {
        let lib = make_sample_library();
        let dir = tempdir().unwrap();
        let path = dir.path().join("sample.gds");
        lib.save(&path).expect("save");

        let polys = load_and_flatten(&path).expect("load+flatten");
        // Expect 2 polygons: TOP's layer-2 rect + the SREF'd CHILD rect on layer 1.
        assert_eq!(polys.len(), 2, "polys: {:?}", polys);

        let on_layer = |l: i16| polys.iter().find(|p| p.layer == l).unwrap();
        let top_rect = on_layer(2);
        assert_eq!(top_rect.points.len(), 4);

        let child_rect = on_layer(1);
        // Child rect was (0,0)-(1000,500), translated by (2000, 0) → (2000,0)-(3000,500).
        let xs: Vec<f64> = child_rect.points.iter().map(|p| p[0]).collect();
        let ys: Vec<f64> = child_rect.points.iter().map(|p| p[1]).collect();
        assert!(xs.iter().all(|&x| x >= 2000.0 - 1e-6 && x <= 3000.0 + 1e-6),
                "child xs: {:?}", xs);
        assert!(ys.iter().all(|&y| y >= 0.0 - 1e-6 && y <= 500.0 + 1e-6),
                "child ys: {:?}", ys);

        let b = bbox(&polys);
        assert!((b.min[0] - 0.0).abs() < 1e-6);
        assert!((b.max[0] - 3000.0).abs() < 1e-6);
        assert!((b.min[1] - 0.0).abs() < 1e-6);
        assert!((b.max[1] - 1500.0).abs() < 1e-6);
    }
}
