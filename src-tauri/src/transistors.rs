//! Transistor extraction (H4) — turns geometry + the H3-style net graph
//! into a list of MOSFETs with classified type and source/drain/gate nets.
//!
//! A MOSFET in layout is a `poly` shape crossing a `diffusion` (active)
//! shape: the overlap is the gate/channel, and the diffusion on either
//! side of it is the source and the drain. H3 deliberately keeps
//! poly-over-diffusion *un*merged (it's a switch, not a wire) but it
//! still treats the whole diffusion rectangle as one net — so a source
//! contact and a drain contact on the same diffusion look like one node.
//! That is wrong for a transistor: the channel separates them.
//!
//! So extraction refines connectivity. For every diffusion that has a
//! gate over it we split the rectangle at each gate, *drop the strip
//! under the gate*, and keep the conducting regions on either side as
//! separate diffusion pieces. Re-running the net builder over that
//! refined polygon set ([`Extraction::device_nets`]) makes source and
//! drain fall into distinct nets — and, for a series stack sharing one
//! diffusion (a NAND/NOR pulldown), the diffusion *between* two gates
//! becomes its own internal net automatically.
//!
//! Classification: a gate whose channel centre sits inside an `nwell`
//! polygon is a PMOS, otherwise an NMOS.
//!
//! Geometry note: detection + splitting assume Manhattan (axis-aligned)
//! poly/diffusion, which is universal for digital standard cells. The
//! gate region is the rectangle intersection of the two bounding boxes,
//! guarded by a point-in-polygon test at its centre so non-overlapping
//! L-shapes whose bboxes merely cross don't register as devices.

use std::collections::HashMap;

use serde::Serialize;

use crate::gds::{self, Bbox, Polygon};
use crate::geometry::{self, Nets};
use crate::tech::{LayerRole, Tech};

const TOL: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransistorKind {
    Nmos,
    Pmos,
}

/// One extracted MOSFET.
#[derive(Debug, Clone, Serialize)]
pub struct Transistor {
    pub kind: TransistorKind,
    /// Index into the original flat polygon list of the gate poly.
    pub poly_index: usize,
    /// Index into the original flat polygon list of the diffusion.
    pub diff_index: usize,
    /// Channel (gate ∩ diffusion) bbox, database units — also the
    /// highlight rectangle the UI draws when a device row is clicked.
    pub gate_min: [f64; 2],
    pub gate_max: [f64; 2],
    /// Gate net (the poly's net in [`Extraction::device_nets`]).
    pub gate_net: u32,
    /// Source / drain device-net ids. `None` when that side of the gate
    /// has no diffusion region (a gate flush with the diffusion edge).
    pub source_net: Option<u32>,
    pub drain_net: Option<u32>,
}

/// Result of H4 extraction over a loaded layout.
#[derive(Debug, Default)]
pub struct Extraction {
    pub transistors: Vec<Transistor>,
    /// Refined connectivity: like the H3 nets but each gated diffusion is
    /// split at its gate(s) so source and drain are distinct nets.
    pub device_nets: Nets,
    /// The polygon list `device_nets` indexes — original polys with every
    /// gated diffusion replaced by its conducting source/drain regions.
    pub device_polys: Vec<Polygon>,
}

/// A gate instance: one poly crossing one diffusion, with the channel
/// region it carves out.
struct Gate {
    poly_index: usize,
    region: Bbox,
}

/// A conducting diffusion region produced by splitting a gated diffusion.
struct Region {
    diff_index: usize,
    bbox: Bbox,
    /// Index of this region's rectangle in `device_polys`.
    device_index: usize,
}

/// Extract transistors from the flattened polygon list under `tech`.
pub fn extract(polys: &[Polygon], tech: &Tech) -> Extraction {
    let role = |i: usize| tech.role(polys[i].layer, polys[i].datatype);
    let poly_idx: Vec<usize> = (0..polys.len()).filter(|&i| role(i) == LayerRole::Poly).collect();
    let diff_idx: Vec<usize> =
        (0..polys.len()).filter(|&i| role(i) == LayerRole::Diffusion).collect();
    let nwell_idx: Vec<usize> =
        (0..polys.len()).filter(|&i| role(i) == LayerRole::NWell).collect();

    // 1. Find every gate (poly × diffusion overlap), grouped per diffusion.
    let mut gates_of: HashMap<usize, Vec<Gate>> = HashMap::new();
    for &di in &diff_idx {
        for &pi in &poly_idx {
            if let Some(region) = gate_region(&polys[pi], &polys[di]) {
                gates_of.entry(di).or_default().push(Gate { poly_index: pi, region });
            }
        }
    }

    // 2. Build the refined "device" polygon list: every non-gated polygon
    //    copied through, every gated diffusion replaced by its conducting
    //    regions (channel strips dropped).
    let mut device_polys: Vec<Polygon> = Vec::with_capacity(polys.len());
    let mut orig_to_device: HashMap<usize, usize> = HashMap::new();
    let mut regions: Vec<Region> = Vec::new();
    for i in 0..polys.len() {
        if role(i) == LayerRole::Diffusion {
            if let Some(gates) = gates_of.get(&i) {
                for rb in split_diffusion(&polys[i], gates) {
                    let device_index = device_polys.len();
                    device_polys.push(rect_polygon(polys[i].layer, polys[i].datatype, &rb));
                    regions.push(Region { diff_index: i, bbox: rb, device_index });
                }
                continue;
            }
        }
        orig_to_device.insert(i, device_polys.len());
        device_polys.push(polys[i].clone());
    }

    // 3. Rebuild nets over the refined polygons.
    let device_nets = geometry::build_nets(&device_polys, tech);

    // 4. Assemble each transistor with its source/drain/gate nets + type.
    let mut transistors = Vec::new();
    for (&di, gates) in &gates_of {
        let axis = long_axis(&gds::polygon_bbox(&polys[di].points));
        for g in gates {
            let gate_net = orig_to_device
                .get(&g.poly_index)
                .map(|&d| device_nets.net_of[d])
                .unwrap_or(0);
            // Source = region whose far edge meets the gate's low edge;
            // drain = region whose near edge meets the gate's high edge.
            let source_net = region_net(&regions, &device_nets, di, axis, g.region.min[axis], true);
            let drain_net = region_net(&regions, &device_nets, di, axis, g.region.max[axis], false);
            let center = rect_center(&g.region);
            let kind = if nwell_idx.iter().any(|&w| gds::point_in_polygon(&polys[w].points, center)) {
                TransistorKind::Pmos
            } else {
                TransistorKind::Nmos
            };
            transistors.push(Transistor {
                kind,
                poly_index: g.poly_index,
                diff_index: di,
                gate_min: g.region.min,
                gate_max: g.region.max,
                gate_net,
                source_net,
                drain_net,
            });
        }
    }
    // Stable order: by diffusion then by gate position. HashMap iteration
    // is unordered, so sort for deterministic output.
    transistors.sort_by(|a, b| {
        a.diff_index
            .cmp(&b.diff_index)
            .then(a.gate_min[0].partial_cmp(&b.gate_min[0]).unwrap())
            .then(a.gate_min[1].partial_cmp(&b.gate_min[1]).unwrap())
    });

    Extraction { transistors, device_nets, device_polys }
}

/// The channel rectangle where `poly` crosses `diff`, or `None` if they
/// don't actually overlap. Exact for axis-aligned rectangles; the centre
/// point-in-polygon guard rejects bbox-only overlaps of non-Manhattan
/// shapes.
fn gate_region(poly: &Polygon, diff: &Polygon) -> Option<Bbox> {
    let inter = rect_intersect(
        &gds::polygon_bbox(&poly.points),
        &gds::polygon_bbox(&diff.points),
    )?;
    let c = rect_center(&inter);
    if gds::point_in_polygon(&poly.points, c) && gds::point_in_polygon(&diff.points, c) {
        Some(inter)
    } else {
        None
    }
}

/// Split a gated diffusion into its conducting regions: the diffusion
/// bbox minus the strips under each gate, cut along the diffusion's long
/// axis. Regions narrower than `TOL` are dropped.
fn split_diffusion(diff: &Polygon, gates: &[Gate]) -> Vec<Bbox> {
    let db = gds::polygon_bbox(&diff.points);
    let axis = long_axis(&db);
    let perp = 1 - axis;
    let mut intervals: Vec<(f64, f64)> =
        gates.iter().map(|g| (g.region.min[axis], g.region.max[axis])).collect();
    intervals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    let mut regions = Vec::new();
    let mut cursor = db.min[axis];
    for (lo, hi) in &intervals {
        if *lo - cursor > TOL {
            regions.push(axis_rect(axis, perp, cursor, *lo, db.min[perp], db.max[perp]));
        }
        cursor = cursor.max(*hi);
    }
    if db.max[axis] - cursor > TOL {
        regions.push(axis_rect(axis, perp, cursor, db.max[axis], db.min[perp], db.max[perp]));
    }
    regions
}

/// Net of the diffusion region of `diff_index` whose edge along `axis`
/// meets coordinate `edge`. `low_side` true → match a region ending at
/// `edge` (source, on the gate's low side); false → a region starting at
/// `edge` (drain, on the high side).
fn region_net(
    regions: &[Region],
    nets: &Nets,
    diff_index: usize,
    axis: usize,
    edge: f64,
    low_side: bool,
) -> Option<u32> {
    regions
        .iter()
        .filter(|r| r.diff_index == diff_index)
        .find(|r| {
            let probe = if low_side { r.bbox.max[axis] } else { r.bbox.min[axis] };
            (probe - edge).abs() < TOL
        })
        .map(|r| nets.net_of[r.device_index])
}

/// 0 if the bbox is wider than tall (gates cut along X), else 1.
fn long_axis(b: &Bbox) -> usize {
    if (b.max[0] - b.min[0]) >= (b.max[1] - b.min[1]) {
        0
    } else {
        1
    }
}

fn rect_center(b: &Bbox) -> [f64; 2] {
    [(b.min[0] + b.max[0]) * 0.5, (b.min[1] + b.max[1]) * 0.5]
}

/// Positive-area intersection of two bboxes.
fn rect_intersect(a: &Bbox, b: &Bbox) -> Option<Bbox> {
    let min = [a.min[0].max(b.min[0]), a.min[1].max(b.min[1])];
    let max = [a.max[0].min(b.max[0]), a.max[1].min(b.max[1])];
    if min[0] < max[0] - TOL && min[1] < max[1] - TOL {
        Some(Bbox { min, max })
    } else {
        None
    }
}

/// Build a bbox from a span `[a0, a1]` along `axis` and `[p0, p1]` along
/// the perpendicular axis.
fn axis_rect(axis: usize, perp: usize, a0: f64, a1: f64, p0: f64, p1: f64) -> Bbox {
    let mut min = [0.0; 2];
    let mut max = [0.0; 2];
    min[axis] = a0;
    max[axis] = a1;
    min[perp] = p0;
    max[perp] = p1;
    Bbox { min, max }
}

fn rect_polygon(layer: i16, datatype: i16, b: &Bbox) -> Polygon {
    Polygon {
        layer,
        datatype,
        points: vec![
            [b.min[0], b.min[1]],
            [b.max[0], b.min[1]],
            [b.max[0], b.max[1]],
            [b.min[0], b.max[1]],
        ],
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn rect(layer: i16, x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
        Polygon { layer, datatype: 0, points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]] }
    }

    /// Layers (default tech): 1 poly, 2 active/diffusion, 3 contact (via),
    /// 4 metal1, 8 nwell.
    ///
    /// Build a full CMOS inverter: shared vertical poly gate crossing a
    /// PMOS active (inside nwell, top) and an NMOS active (bottom); source
    /// & drain contacts up to metal1; the two drains tied by one output
    /// metal strip. Shared with the H5 sim tests via `pub(crate)`.
    pub(crate) fn inverter() -> Vec<Polygon> {
        vec![
            // nwell over the PMOS half.
            rect(8, 0.0, 1000.0, 3000.0, 2300.0),
            // PMOS active (inside nwell) + NMOS active (substrate).
            rect(2, 500.0, 1300.0, 2500.0, 1900.0), // 1: pmos active
            rect(2, 500.0, 100.0, 2500.0, 700.0),   // 2: nmos active
            // Shared poly gate (vertical stripe) = input.
            rect(1, 1300.0, 0.0, 1700.0, 2300.0), // 3: poly
            // Contacts: pmos S (left) / pmos D (right) / nmos D (right) / nmos S (left) / poly.
            rect(3, 700.0, 1500.0, 900.0, 1700.0),   // 4: pmos source contact
            rect(3, 2100.0, 1500.0, 2300.0, 1700.0), // 5: pmos drain contact
            rect(3, 2100.0, 300.0, 2300.0, 500.0),   // 6: nmos drain contact
            rect(3, 700.0, 300.0, 900.0, 500.0),     // 7: nmos source contact
            rect(3, 1400.0, 2050.0, 1600.0, 2250.0), // 8: poly contact (above active)
            // Metal1: VDD pad, GND pad, input pad, and the OUTPUT strip
            // bridging both drain contacts.
            rect(4, 600.0, 1450.0, 1000.0, 1750.0),  // 9: vdd metal
            rect(4, 600.0, 250.0, 1000.0, 550.0),    // 10: gnd metal
            rect(4, 1350.0, 2000.0, 1650.0, 2300.0), // 11: input metal
            rect(4, 2050.0, 300.0, 2350.0, 1700.0),  // 12: output metal (spans both drains)
        ]
    }

    #[test]
    fn extracts_cmos_inverter() {
        let polys = inverter();
        let ext = extract(&polys, Tech::default_tech());
        assert_eq!(ext.transistors.len(), 2, "an inverter has two transistors");

        let pmos = ext.transistors.iter().find(|t| t.kind == TransistorKind::Pmos).unwrap();
        let nmos = ext.transistors.iter().find(|t| t.kind == TransistorKind::Nmos).unwrap();

        // Both gates are the same poly → one shared input net.
        assert_eq!(pmos.gate_net, nmos.gate_net, "gates share the input net");
        assert_eq!(pmos.poly_index, nmos.poly_index);

        // Every terminal resolved.
        for t in [pmos, nmos] {
            assert!(t.source_net.is_some(), "source resolved");
            assert!(t.drain_net.is_some(), "drain resolved");
            assert_ne!(t.source_net, t.drain_net, "source and drain are distinct nets");
            assert_ne!(Some(t.gate_net), t.source_net, "gate isolated from source");
            assert_ne!(Some(t.gate_net), t.drain_net, "gate isolated from drain");
        }

        // The drains are tied together by the output metal strip → the two
        // transistors share exactly one source/drain net (the output node),
        // and their other terminals (VDD vs GND) differ.
        let pmos_terms = [pmos.source_net.unwrap(), pmos.drain_net.unwrap()];
        let nmos_terms = [nmos.source_net.unwrap(), nmos.drain_net.unwrap()];
        let shared: Vec<u32> =
            pmos_terms.iter().copied().filter(|n| nmos_terms.contains(n)).collect();
        assert_eq!(shared.len(), 1, "exactly the output node is shared, got {shared:?}");

        // VDD (pmos non-output) and GND (nmos non-output) are different nets.
        let pmos_supply = *pmos_terms.iter().find(|n| !shared.contains(n)).unwrap();
        let nmos_supply = *nmos_terms.iter().find(|n| !shared.contains(n)).unwrap();
        assert_ne!(pmos_supply, nmos_supply, "VDD and GND are separate");
    }

    #[test]
    fn series_stack_shares_an_internal_node() {
        // One NMOS diffusion crossed by TWO poly gates → two transistors in
        // series, sharing the middle diffusion region as an internal net.
        let polys = vec![
            rect(2, 0.0, 0.0, 3000.0, 600.0),    // 0: active
            rect(1, 800.0, -100.0, 1000.0, 700.0), // 1: gate A
            rect(1, 2000.0, -100.0, 2200.0, 700.0), // 2: gate B
            // Contacts at both ends + the middle, each up to its own metal.
            rect(3, 300.0, 200.0, 500.0, 400.0),   // 3: left contact
            rect(3, 1400.0, 200.0, 1600.0, 400.0), // 4: middle contact
            rect(3, 2500.0, 200.0, 2700.0, 400.0), // 5: right contact
            rect(4, 250.0, 150.0, 550.0, 450.0),   // 6
            rect(4, 1350.0, 150.0, 1650.0, 450.0), // 7
            rect(4, 2450.0, 150.0, 2750.0, 450.0), // 8
        ];
        let ext = extract(&polys, Tech::default_tech());
        assert_eq!(ext.transistors.len(), 2);
        // Both NMOS (no nwell).
        assert!(ext.transistors.iter().all(|t| t.kind == TransistorKind::Nmos));

        let a = &ext.transistors[0]; // sorted by gate x → gate A first
        let b = &ext.transistors[1];
        assert!(a.gate_min[0] < b.gate_min[0]);
        // A's drain (high side) == B's source (low side): the shared middle.
        assert_eq!(a.drain_net, b.source_net, "series pair shares the middle node");
        // The outer terminals differ from the shared middle.
        assert_ne!(a.source_net, a.drain_net);
        assert_ne!(b.source_net, b.drain_net);
        assert_ne!(a.source_net, b.drain_net, "the two outer ends are distinct");
    }

    #[test]
    fn extracts_from_the_committed_inverter_gds() {
        // The on-disk fixture (testdata/inverter.gds, written by
        // `cargo run --example make_inverter_gds`) must parse through the
        // real load path and extract the same 1 NMOS + 1 PMOS the
        // in-memory twin does. Guards the round-trip end-to-end.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("testdata")
            .join("inverter.gds");
        if !path.exists() {
            return; // fixture not generated in this checkout — skip.
        }
        let info = crate::gds::load_and_flatten(&path).expect("load inverter.gds");
        let ext = extract(&info.polygons, Tech::default_tech());
        assert_eq!(ext.transistors.len(), 2);
        assert_eq!(ext.transistors.iter().filter(|t| t.kind == TransistorKind::Nmos).count(), 1);
        assert_eq!(ext.transistors.iter().filter(|t| t.kind == TransistorKind::Pmos).count(), 1);
    }

    #[test]
    fn no_transistors_without_poly_over_diffusion() {
        // Diffusion + a metal wire crossing it, but no poly → no devices.
        let polys = vec![
            rect(2, 0.0, 0.0, 1000.0, 200.0),
            rect(4, 400.0, -100.0, 600.0, 300.0),
        ];
        let ext = extract(&polys, Tech::default_tech());
        assert!(ext.transistors.is_empty());
    }
}
