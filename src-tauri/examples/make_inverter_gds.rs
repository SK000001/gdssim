//! Writes a synthetic CMOS inverter `.gds` for exercising the H4
//! transistor extractor in the running app (the photonic dies have no
//! transistors). The layout matches `transistors.rs`'s `inverter()` test
//! fixture exactly, so what the unit test asserts is what you can open
//! and click in `pnpm tauri dev`.
//!
//! Layers follow the default tech (`tech/default.json`):
//!   1 poly · 2 active/diffusion · 3 contact · 4 metal1 · 8 nwell
//!
//! Run:  cargo run --example make_inverter_gds [out.gds]
//! Default output: ../testdata/inverter.gds (repo testdata dir).

use std::path::PathBuf;

use gds21::{GdsBoundary, GdsElement, GdsLibrary, GdsPoint, GdsStruct, GdsUnits};

fn rect(layer: i16, x0: i32, y0: i32, x1: i32, y1: i32) -> GdsElement {
    GdsElement::GdsBoundary(GdsBoundary {
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
    })
}

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("testdata")
                .join("inverter.gds")
        });
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir).expect("create testdata dir");
    }

    let inverter = GdsStruct {
        name: "INVERTER".into(),
        elems: vec![
            rect(8, 0, 1000, 3000, 2300),    // nwell over the PMOS half
            rect(2, 500, 1300, 2500, 1900),  // PMOS active (inside nwell)
            rect(2, 500, 100, 2500, 700),    // NMOS active (substrate)
            rect(1, 1300, 0, 1700, 2300),    // shared poly gate = input
            rect(3, 700, 1500, 900, 1700),   // PMOS source contact
            rect(3, 2100, 1500, 2300, 1700), // PMOS drain contact
            rect(3, 2100, 300, 2300, 500),   // NMOS drain contact
            rect(3, 700, 300, 900, 500),     // NMOS source contact
            rect(3, 1400, 2050, 1600, 2250), // poly (gate) contact
            rect(4, 600, 1450, 1000, 1750),  // VDD metal
            rect(4, 600, 250, 1000, 550),    // GND metal
            rect(4, 1350, 2000, 1650, 2300), // input metal
            rect(4, 2050, 300, 2350, 1700),  // OUTPUT metal (bridges both drains)
        ],
        ..Default::default()
    };

    let lib = GdsLibrary {
        name: "CMOS_INVERTER".into(),
        units: GdsUnits::new(0.001, 1e-9), // 1 DB unit = 1 nm
        structs: vec![inverter],
        ..Default::default()
    };

    lib.save(&out).expect("save gds");
    println!("wrote {}", out.display());
}
