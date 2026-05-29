//! Writes the H8 "circuit zoo" as `.gds` files for exercising the
//! extractor + simulator in the running app: NAND2, NOR2, SR latch
//! (the inverter has its own example). Each layout is generated from a
//! transistor netlist by `celllib::build` (gate-matrix routing), so what
//! the unit tests assert is what you can open and simulate.
//!
//! Run:  cargo run --example make_cells
//! Output: ../testdata/<cell>.gds

use std::path::PathBuf;

use gdssim_lib::celllib;
use gdssim_lib::gds::Polygon;
use gds21::{GdsBoundary, GdsElement, GdsLibrary, GdsPoint, GdsStruct, GdsUnits};

fn to_boundary(p: &Polygon) -> GdsElement {
    let mut xy: Vec<GdsPoint> = p
        .points
        .iter()
        .map(|pt| GdsPoint::new(pt[0].round() as i32, pt[1].round() as i32))
        .collect();
    if let Some(first) = xy.first().cloned() {
        xy.push(first); // close the ring
    }
    GdsElement::GdsBoundary(GdsBoundary { layer: p.layer, datatype: p.datatype, xy, ..Default::default() })
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("testdata");
    std::fs::create_dir_all(&dir).expect("create testdata dir");

    for cell in celllib::zoo() {
        let (polys, _probes) = celllib::build(&cell);
        let strukt = GdsStruct {
            name: cell.name.into(),
            elems: polys.iter().map(to_boundary).collect(),
            ..Default::default()
        };
        let lib = GdsLibrary {
            name: cell.name.into(),
            units: GdsUnits::new(0.001, 1e-9), // 1 DB unit = 1 nm
            structs: vec![strukt],
            ..Default::default()
        };
        let out = dir.join(format!("{}.gds", cell.name.to_lowercase()));
        lib.save(&out).expect("save gds");
        println!("wrote {} ({} polys)", out.display(), polys.len());
    }
}
