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
///
/// Real-world GDS files (especially gdsfactory output) sometimes
/// carry zero / out-of-range Y/M/D fields in BGNLIB and BGNSTR.
/// gds21 hands those to `chrono::NaiveDate::from_ymd_opt(...).unwrap()`
/// which panics. We patch those record bodies to a safe sentinel date
/// (1980-01-01 00:00:00) before parsing — gds21 only uses the dates
/// for library metadata, which we discard anyway.
pub fn load_and_flatten(path: &Path) -> Result<Vec<Polygon>, GdsError> {
    let bytes = std::fs::read(path).map_err(|e| GdsError::Parse(format!("read: {e}")))?;
    let patched = sanitize_dates(&bytes);
    let tmp = tempfile::Builder::new()
        .prefix("gdssim-sanitized-")
        .suffix(".gds")
        .tempfile()
        .map_err(|e| GdsError::Parse(format!("tempfile: {e}")))?;
    std::fs::write(tmp.path(), &patched)
        .map_err(|e| GdsError::Parse(format!("tempfile write: {e}")))?;
    let lib = GdsLibrary::load(tmp.path()).map_err(|e| GdsError::Parse(format!("{e:?}")))?;
    flatten_library(&lib)
}

/// Walk the GDS record stream and overwrite the 12 i16 date words of
/// every BGNLIB (record 0x01) and BGNSTR (record 0x05) with
/// 1980-01-01 00:00:00.
///
/// GDS record format: `[u16 BE length][u8 rec_type][u8 data_type][data…]`.
/// `length` includes the 4-byte header. We only mutate the data block.
fn sanitize_dates(input: &[u8]) -> Vec<u8> {
    const REC_BGNLIB: u8 = 0x01;
    const REC_BGNSTR: u8 = 0x05;
    // Two dates, Y/M/D/h/m/s each, as i16 BE.
    const SAFE_DATE_BYTES: [u8; 24] = [
        0x07, 0xBC, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x07, 0xBC, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    let mut out = input.to_vec();
    let mut i = 0;
    while i + 4 <= out.len() {
        let len = u16::from_be_bytes([out[i], out[i + 1]]) as usize;
        if len < 4 || i + len > out.len() {
            break; // malformed; bail without further mutation
        }
        let rec_type = out[i + 2];
        if rec_type == REC_BGNLIB || rec_type == REC_BGNSTR {
            let body_start = i + 4;
            let body_end = i + len;
            let body_len = body_end - body_start;
            if body_len >= SAFE_DATE_BYTES.len() {
                out[body_start..body_start + SAFE_DATE_BYTES.len()]
                    .copy_from_slice(&SAFE_DATE_BYTES);
            }
        }
        i += len;
    }
    out
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

/// Extend the start / end of a path centerline according to its
/// `path_type`. Type 0 = flush (no extension); type 1 = round (treat
/// as flush — we'd need an arc otherwise); type 2 = square extended
/// by half-width; type 4 = custom extensions from `BGNEXTN` / `ENDEXTN`.
fn extend_path_endpoints(
    mut pts: Vec<[f64; 2]>,
    path_type: i16,
    width: f64,
    bgn_extn: f64,
    end_extn: f64,
) -> Vec<[f64; 2]> {
    if pts.len() < 2 {
        return pts;
    }
    let (bgn_amt, end_amt) = match path_type {
        2 => (width * 0.5, width * 0.5),
        4 => (bgn_extn, end_extn),
        _ => (0.0, 0.0),
    };
    if bgn_amt > 0.0 {
        let d = sub2(pts[1], pts[0]);
        let n = norm2(d);
        pts[0] = [pts[0][0] - n[0] * bgn_amt, pts[0][1] - n[1] * bgn_amt];
    }
    if end_amt > 0.0 {
        let last = pts.len() - 1;
        let d = sub2(pts[last], pts[last - 1]);
        let n = norm2(d);
        pts[last] = [pts[last][0] + n[0] * end_amt, pts[last][1] + n[1] * end_amt];
    }
    pts
}

/// Offset a polyline by ±width/2 along the bisector at each vertex,
/// producing a closed ring (left forward + right reversed). Miter
/// length is capped at 4× half-width to avoid sharp-angle spikes.
fn path_to_polygon(centerline: &[[f64; 2]], width: f64) -> Vec<[f64; 2]> {
    let hw = width * 0.5;
    let n = centerline.len();
    if n < 2 || hw <= 0.0 {
        return vec![];
    }
    let mut left = Vec::with_capacity(n);
    let mut right = Vec::with_capacity(n);
    let max_miter = hw * 4.0;
    for i in 0..n {
        let dir_in = if i == 0 {
            sub2(centerline[1], centerline[0])
        } else {
            sub2(centerline[i], centerline[i - 1])
        };
        let dir_out = if i == n - 1 {
            sub2(centerline[n - 1], centerline[n - 2])
        } else {
            sub2(centerline[i + 1], centerline[i])
        };
        let n_in = norm2(dir_in);
        let n_out = norm2(dir_out);
        let perp_in = [-n_in[1], n_in[0]];
        let perp_out = [-n_out[1], n_out[0]];
        let bisect_raw = [perp_in[0] + perp_out[0], perp_in[1] + perp_out[1]];
        let bisect = norm2(bisect_raw);
        let dot = bisect[0] * perp_in[0] + bisect[1] * perp_in[1];
        let miter = if dot.abs() > 1e-3 { hw / dot } else { max_miter };
        let m = miter.abs().min(max_miter);
        left.push([centerline[i][0] + bisect[0] * m, centerline[i][1] + bisect[1] * m]);
        right.push([centerline[i][0] - bisect[0] * m, centerline[i][1] - bisect[1] * m]);
    }
    let mut polygon = left;
    polygon.extend(right.iter().rev().copied());
    polygon
}

fn sub2(a: [f64; 2], b: [f64; 2]) -> [f64; 2] { [a[0] - b[0], a[1] - b[1]] }

fn norm2(v: [f64; 2]) -> [f64; 2] {
    let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if len < 1e-12 { [0.0, 0.0] } else { [v[0] / len, v[1] / len] }
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
            GdsElement::GdsPath(p) => {
                let local: Vec<[f64; 2]> = p.xy.iter().map(|q| [q.x as f64, q.y as f64]).collect();
                let width = p.width.unwrap_or(0) as f64;
                if width <= 0.0 || local.len() < 2 { continue; }
                let path_type = p.path_type.unwrap_or(0);
                let bgn_extn = p.begin_extn.unwrap_or(0) as f64;
                let end_extn = p.end_extn.unwrap_or(0) as f64;
                let extended = extend_path_endpoints(local, path_type, width, bgn_extn, end_extn);
                let poly_local = path_to_polygon(&extended, width);
                if poly_local.len() < 3 { continue; }
                let poly: Vec<[f64; 2]> = poly_local.into_iter().map(|pt| parent.apply(pt)).collect();
                out.push(Polygon {
                    layer: p.layer,
                    datatype: p.datatype,
                    points: poly,
                });
            }
            GdsElement::GdsTextElem(_) | GdsElement::GdsNode(_) => {}
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
    fn path_to_polygon_horizontal_segment() {
        // 1000-unit horizontal segment, width 100 → expect 200×1000 polygon.
        let centerline = vec![[0.0, 0.0], [1000.0, 0.0]];
        let poly = path_to_polygon(&centerline, 100.0);
        assert_eq!(poly.len(), 4);
        let xs: Vec<f64> = poly.iter().map(|p| p[0]).collect();
        let ys: Vec<f64> = poly.iter().map(|p| p[1]).collect();
        let xmin = xs.iter().cloned().fold(f64::INFINITY, f64::min);
        let xmax = xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let ymin = ys.iter().cloned().fold(f64::INFINITY, f64::min);
        let ymax = ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        assert!((xmin - 0.0).abs() < 1e-6);
        assert!((xmax - 1000.0).abs() < 1e-6);
        assert!((ymin - -50.0).abs() < 1e-6);
        assert!((ymax - 50.0).abs() < 1e-6);
    }

    #[test]
    fn extend_path_endpoints_square_cap() {
        // path_type 2 should extend each end by width/2 along its tangent.
        let pts = vec![[0.0, 0.0], [1000.0, 0.0]];
        let extended = extend_path_endpoints(pts, 2, 100.0, 0.0, 0.0);
        assert!((extended[0][0] - -50.0).abs() < 1e-6);
        assert!((extended[1][0] - 1050.0).abs() < 1e-6);
    }

    #[test]
    fn sanitize_dates_replaces_bgnlib_and_bgnstr_bodies() {
        // Craft a minimal stream: BGNLIB(year=0) + BGNSTR(year=99999&0xFFFF) +
        // an unrelated record we shouldn't touch.
        let mut buf = Vec::new();
        // BGNLIB: length=28, rec=0x01, dtype=0x02, 12 i16 (all zero → would panic chrono).
        buf.extend_from_slice(&28u16.to_be_bytes());
        buf.push(0x01);
        buf.push(0x02);
        buf.extend_from_slice(&[0u8; 24]);
        // BGNSTR: same shape, year=9999 (still out of typical range).
        buf.extend_from_slice(&28u16.to_be_bytes());
        buf.push(0x05);
        buf.push(0x02);
        buf.extend_from_slice(&9999i16.to_be_bytes());
        buf.extend_from_slice(&[0u8; 22]);
        // Untouched record: STRNAME (rec=0x06, dtype=0x06 ASCII), 4 chars.
        buf.extend_from_slice(&8u16.to_be_bytes());
        buf.push(0x06);
        buf.push(0x06);
        buf.extend_from_slice(b"TOP_");

        let out = sanitize_dates(&buf);
        // BGNLIB body now starts with year=1980 (0x07BC).
        assert_eq!(&out[4..6], &[0x07, 0xBC]);
        // BGNSTR body also patched.
        assert_eq!(&out[32..34], &[0x07, 0xBC]);
        // STRNAME unchanged.
        assert_eq!(&out[60..64], b"TOP_");
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
