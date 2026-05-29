//! Synthetic CMOS cell library (H8 "circuit zoo").
//!
//! Hand-placing standard cells in coordinates is fiddly and easy to get
//! wrong, so instead we generate a layout from a transistor *netlist*
//! using a simple, provably non-conflicting gate-matrix scheme:
//!
//!   - Each transistor sits in its own column: a diffusion rectangle with
//!     a poly gate across the middle (so the extractor sees one device,
//!     source = left region, drain = right region).
//!   - Each net gets a horizontal metal2 *track* at a distinct y. Tracks
//!     never touch each other.
//!   - Each terminal (source / drain / gate) taps its net's track through
//!     a contact (→ metal1) → a vertical metal1 stub → a via (→ metal2).
//!     Stubs sit at distinct x columns, so they never touch each other,
//!     and a stub crossing an unrelated track is metal1-over-metal2 with
//!     no via — no connection.
//!
//! The result isn't a compact standard cell, but it's a valid, fully
//! extractable + simulatable circuit (and a clear visual of the netlist),
//! which is exactly what the zoo + the H4/H5 tests need.
//!
//! Layers follow the default tech: 1 poly · 2 active · 3 contact · 4
//! metal1 · 5 metal2 · 7 via · 8 nwell.

use std::collections::HashMap;

use crate::gds::Polygon;

/// One transistor in a netlist: type, gate net, and its two source/drain
/// nets (order is irrelevant — the device is symmetric).
pub struct Mos {
    pub p: bool,
    pub gate: &'static str,
    pub a: &'static str,
    pub b: &'static str,
}

/// A cell = a name + its transistors, wired by net name.
pub struct Cell {
    pub name: &'static str,
    pub mos: Vec<Mos>,
}

const COL_W: f64 = 2000.0; // column pitch (> DIFF_W so columns don't touch)
const MARGIN: f64 = 400.0;
const DIFF_W: f64 = 1200.0;
const DIFF_Y1: f64 = 600.0;
const TRACK_Y0: f64 = 1200.0; // first track sits above the gate contacts
const TRACK_PITCH: f64 = 400.0;
const TRACK_H: f64 = 200.0;

fn rect(layer: i16, x0: f64, y0: f64, x1: f64, y1: f64) -> Polygon {
    Polygon { layer, datatype: 0, points: vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]] }
}

/// Build the layout for `cell`. Returns the polygons plus, for each net,
/// a probe point on its track (so callers can map a net name → the
/// extractor's device net via `device_net_at`).
pub fn build(cell: &Cell) -> (Vec<Polygon>, Vec<(&'static str, [f64; 2])>) {
    // Distinct nets in first-seen order → track index.
    let mut order: Vec<&'static str> = Vec::new();
    let mut idx: HashMap<&'static str, usize> = HashMap::new();
    for m in &cell.mos {
        for n in [m.gate, m.a, m.b] {
            if !idx.contains_key(n) {
                idx.insert(n, order.len());
                order.push(n);
            }
        }
    }

    let ncols = cell.mos.len();
    let ytrack = |i: usize| TRACK_Y0 + (i as f64) * TRACK_PITCH;
    let x_left = -200.0;
    let x_right = MARGIN + (ncols as f64) * COL_W + DIFF_W + 200.0;

    let mut polys = Vec::new();
    let mut probes = Vec::new();

    // Net tracks (metal2) + their probe points.
    for (i, &name) in order.iter().enumerate() {
        let y = ytrack(i);
        polys.push(rect(5, x_left, y, x_right, y + TRACK_H));
        probes.push((name, [0.0, y + TRACK_H * 0.5]));
    }

    // Connect a terminal at column `x` (metal1 stub `sx0..sx1`, from
    // `y_bottom`) up to net `net`'s track, with a via.
    let mut tap = |polys: &mut Vec<Polygon>, sx0: f64, sx1: f64, y_bottom: f64, net: usize| {
        let yt = ytrack(net);
        polys.push(rect(4, sx0, y_bottom, sx1, yt + TRACK_H)); // metal1 stub
        polys.push(rect(7, sx0, yt, sx1, yt + TRACK_H)); // via to metal2 track
    };

    for (col, m) in cell.mos.iter().enumerate() {
        let x0 = MARGIN + (col as f64) * COL_W;
        let gx0 = x0 + 500.0;
        let gx1 = x0 + 700.0;

        polys.push(rect(2, x0, 0.0, x0 + DIFF_W, DIFF_Y1)); // diffusion
        if m.p {
            polys.push(rect(8, x0 - 100.0, -300.0, x0 + DIFF_W + 100.0, 900.0)); // nwell
        }
        polys.push(rect(1, gx0, -200.0, gx1, 1000.0)); // poly gate

        // Contacts: source (left region) / drain (right region) / gate.
        polys.push(rect(3, x0 + 200.0, 200.0, x0 + 400.0, 400.0));
        polys.push(rect(3, x0 + 850.0, 200.0, x0 + 1050.0, 400.0));
        polys.push(rect(3, gx0, 800.0, gx1, 1000.0));

        // Route each terminal to its net track.
        tap(&mut polys, x0 + 200.0, x0 + 400.0, 200.0, idx[m.a]); // source
        tap(&mut polys, x0 + 850.0, x0 + 1050.0, 200.0, idx[m.b]); // drain
        tap(&mut polys, gx0, gx1, 800.0, idx[m.gate]); // gate
    }

    (polys, probes)
}

// ---- the zoo ----

/// 2-input NAND: series NMOS pulldown, parallel PMOS pullup.
pub fn nand2() -> Cell {
    Cell {
        name: "NAND2",
        mos: vec![
            Mos { p: true, gate: "A", a: "VDD", b: "OUT" },
            Mos { p: true, gate: "B", a: "VDD", b: "OUT" },
            Mos { p: false, gate: "A", a: "GND", b: "NINT" },
            Mos { p: false, gate: "B", a: "NINT", b: "OUT" },
        ],
    }
}

/// 2-input NOR: series PMOS pullup, parallel NMOS pulldown.
pub fn nor2() -> Cell {
    Cell {
        name: "NOR2",
        mos: vec![
            Mos { p: true, gate: "A", a: "VDD", b: "PINT" },
            Mos { p: true, gate: "B", a: "PINT", b: "OUT" },
            Mos { p: false, gate: "A", a: "GND", b: "OUT" },
            Mos { p: false, gate: "B", a: "GND", b: "OUT" },
        ],
    }
}

/// SR latch from cross-coupled NOR gates:
/// Q = NOR(R, QBAR), QBAR = NOR(S, Q).
pub fn sr_latch() -> Cell {
    Cell {
        name: "SR_LATCH",
        mos: vec![
            // Q = NOR(R, QBAR)
            Mos { p: true, gate: "R", a: "VDD", b: "PA" },
            Mos { p: true, gate: "QBAR", a: "PA", b: "Q" },
            Mos { p: false, gate: "R", a: "GND", b: "Q" },
            Mos { p: false, gate: "QBAR", a: "GND", b: "Q" },
            // QBAR = NOR(S, Q)
            Mos { p: true, gate: "S", a: "VDD", b: "PB" },
            Mos { p: true, gate: "Q", a: "PB", b: "QBAR" },
            Mos { p: false, gate: "S", a: "GND", b: "QBAR" },
            Mos { p: false, gate: "Q", a: "GND", b: "QBAR" },
        ],
    }
}

/// Every zoo cell, for the GDS-writing example.
pub fn zoo() -> Vec<Cell> {
    vec![nand2(), nor2(), sr_latch()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{simulate, Logic};
    use crate::tech::Tech;
    use crate::transistors::{device_net_at, extract, Extraction, TransistorKind};

    fn extract_cell(cell: &Cell) -> (Extraction, HashMap<&'static str, u32>) {
        let (polys, probes) = build(cell);
        let ext = extract(&polys, Tech::default_tech());
        let mut name2net = HashMap::new();
        for (n, p) in &probes {
            name2net.insert(*n, device_net_at(&ext, *p).expect("net at probe point"));
        }
        (ext, name2net)
    }

    fn run(cell: &Cell, fixed_names: &[(&str, Logic)]) -> (Vec<Logic>, HashMap<&'static str, u32>) {
        let (ext, name2net) = extract_cell(cell);
        let mut fixed = HashMap::new();
        for (n, v) in fixed_names {
            fixed.insert(name2net[n], *v);
        }
        let vals = simulate(&ext.transistors, ext.device_nets.count(), &fixed);
        (vals, name2net)
    }

    fn count_kinds(ext: &Extraction) -> (usize, usize) {
        let p = ext.transistors.iter().filter(|t| t.kind == TransistorKind::Pmos).count();
        let n = ext.transistors.iter().filter(|t| t.kind == TransistorKind::Nmos).count();
        (p, n)
    }

    #[test]
    fn nand2_extracts_and_computes() {
        let (ext, _) = extract_cell(&nand2());
        assert_eq!(ext.transistors.len(), 4);
        assert_eq!(count_kinds(&ext), (2, 2));

        use Logic::{One, Zero};
        let table = [
            (Zero, Zero, One),
            (Zero, One, One),
            (One, Zero, One),
            (One, One, Zero),
        ];
        for (a, b, out) in table {
            let (v, n) = run(&nand2(), &[("VDD", One), ("GND", Zero), ("A", a), ("B", b)]);
            assert_eq!(v[n["OUT"] as usize], out, "NAND({a:?},{b:?})");
        }
    }

    #[test]
    fn nor2_extracts_and_computes() {
        let (ext, _) = extract_cell(&nor2());
        assert_eq!(ext.transistors.len(), 4);
        assert_eq!(count_kinds(&ext), (2, 2));

        use Logic::{One, Zero};
        let table = [
            (Zero, Zero, One),
            (Zero, One, Zero),
            (One, Zero, Zero),
            (One, One, Zero),
        ];
        for (a, b, out) in table {
            let (v, n) = run(&nor2(), &[("VDD", One), ("GND", Zero), ("A", a), ("B", b)]);
            assert_eq!(v[n["OUT"] as usize], out, "NOR({a:?},{b:?})");
        }
    }

    #[test]
    fn committed_nand2_gds_round_trips() {
        // The on-disk fixture (written by `cargo run --example make_cells`)
        // must parse through the real load path and extract 4 transistors,
        // guarding the generator's GDS output end-to-end.
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("testdata")
            .join("nand2.gds");
        if !path.exists() {
            return; // fixture not generated in this checkout — skip.
        }
        let info = crate::gds::load_and_flatten(&path).expect("load nand2.gds");
        let ext = extract(&info.polygons, Tech::default_tech());
        assert_eq!(ext.transistors.len(), 4);
        assert_eq!(count_kinds(&ext), (2, 2));
    }

    #[test]
    fn sr_latch_set_and_reset() {
        let (ext, _) = extract_cell(&sr_latch());
        assert_eq!(ext.transistors.len(), 8);
        assert_eq!(count_kinds(&ext), (4, 4));

        use Logic::{One, Zero};
        // Set: S=1, R=0 → Q=1, QBAR=0.
        let (v, n) = run(&sr_latch(), &[("VDD", One), ("GND", Zero), ("S", One), ("R", Zero)]);
        assert_eq!(v[n["Q"] as usize], One);
        assert_eq!(v[n["QBAR"] as usize], Zero);
        // Reset: S=0, R=1 → Q=0, QBAR=1.
        let (v, n) = run(&sr_latch(), &[("VDD", One), ("GND", Zero), ("S", Zero), ("R", One)]);
        assert_eq!(v[n["Q"] as usize], Zero);
        assert_eq!(v[n["QBAR"] as usize], One);
    }
}
