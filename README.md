# GDSSIM

Interactive GDS-layout simulator — a subproject of TERN.

Load a `.gds` chip layout → auto-extract the transistor graph → run a
digital sim → animate signal flow over the real geometry.
Educational-first: a visual transistor explorer and reverse-engineering
tool, not an EDA replacement.

See [Track H in `../roadmap.md`](../roadmap.md) for the full 7-phase
plan; [`../workflow.md`](../workflow.md) for current session state.

## Stack

| Layer       | Tech                                          |
| ----------- | --------------------------------------------- |
| UI          | React + TypeScript                            |
| Desktop     | Tauri 2                                        |
| Backend     | Rust                                          |
| Rendering   | WebGPU (browser, in a `<canvas>`)             |
| Parallelism | Rayon (later)                                 |

**Single-window architecture (since H1.5).** Everything lives in one
Tauri webview. Rust does the heavy lifting — GDS parsing, hierarchy
flattening, polygon triangulation, the geometry/net graph, transistor
extraction, and the switch-level sim — and ships ready-to-upload
vertex/index buffers + results to the frontend over Tauri IPC. The
frontend renders them with **browser WebGPU** into a `<canvas>` and owns
the camera, picking, and the animated overlays. (There is no Rust-owned
`winit`/`wgpu` window; that early approach was dropped in H1.5.)

## Status

Phases 2–6 and the H8 circuit-zoo core are done. The full pipeline
works: **load → view → select → extract transistors → simulate →
animate**, verified on synthetic CMOS cells and real photonic dies.

- **Load + view (H2).** `gds21` parser, hierarchy flatten (SREF/AREF
  affine compose, `PATH`→polygon, out-of-range date sanitizer,
  last-unreferenced-cell top detect). Per-layer render with 4× MSAA +
  outlines; layer-visibility toggle; wheel-zoom / middle-drag / `F` fit /
  `+/-`.
- **Tech file (H9, build-time half).** `(layer, datatype) → name +
  colour + electrical role` from an embedded JSON (`src-tauri/tech/`).
- **Select (H2d).** Left-click → smallest containing polygon → inspector
  panel + whole-net highlight (translucent fill under a haloed outline).
- **Net graph (H3).** Uniform-grid index → polygon-touch → Union-Find;
  vias bridge layers, wells are inert.
- **Transistor extraction (H4).** Each poly×diffusion overlap → a MOSFET,
  NMOS/PMOS by n-well containment; source/drain/gate nets resolved by
  splitting each gated diffusion at its gate(s) (series stacks share an
  internal node). Devices panel.
- **Digital sim (H5).** Three-valued (0/1/X) switch-level solver. A
  **Simulate** mode: click a net to tag it VDD/GND/input, toggle inputs,
  and the layout recolours live by solved value.
- **Animation (H6).** Logic-1 nets flow with scrolling stripes, logic-0
  flat blue, X grey, conducting transistors glow green.
- **Circuit zoo (H8).** `celllib.rs` generates layouts from transistor
  netlists; `testdata/` holds inverter, NAND2, NOR2, and an SR latch.

## Prerequisites

- Rust ≥ 1.85 (Tauri 2 deps require edition 2024). `rustup update stable`.
- Node ≥ 18, pnpm ≥ 9.
- Windows: WebView2 runtime (preinstalled on Windows 11). WebGPU must be
  available in the WebView2 build.
- A GPU + driver with Vulkan / DX12 / Metal support.

## Run

```
pnpm install
pnpm tauri dev
```

This starts Vite on http://localhost:1420, compiles the Rust crate, and
launches the Tauri window. Then:

1. **Open .gds file…** and pick a layout (try `testdata/inverter.gds` or
   the zoo cells below).
2. Left-click a shape to inspect it + highlight its net.
3. **Simulate** → click a net and tag it **VDD** / **GND** / **input**,
   toggle the inputs, and watch the layout light up (red 1 / blue 0 /
   grey X, flowing stripes on driven nets, green glow on conducting
   transistors).

### Test layouts

Synthetic CMOS cells live in `testdata/` (regenerate with the examples):

```
cargo run --example make_inverter_gds   # testdata/inverter.gds
cargo run --example make_cells          # nand2 / nor2 / sr_latch .gds
```

(run from `src-tauri/`). Each is generated from a transistor netlist by
`celllib.rs`, so what the unit tests assert is what you can open and
simulate.

## Layout

```
gdssim/
├─ src/                  React + TS frontend (Tauri webview)
│  ├─ App.tsx              top bar + cells/layer/devices/sim panels + inspector
│  ├─ viewport.ts          WebGPU pipelines, camera, picking, highlight + sim overlay
│  ├─ viewport.wgsl        fill / edge / highlight / animated-flow shaders
│  ├─ App.css, main.tsx, vite-env.d.ts
├─ src-tauri/            Rust crate (Tauri backend + all compute)
│  ├─ src/
│  │  ├─ main.rs           thin entry → gdssim_lib::run()
│  │  ├─ lib.rs            Tauri builder + IPC (load_gds, hit_test, net_rings,
│  │  │                    transistors, device_net(s)_*, simulate_nets)
│  │  ├─ gds.rs            gds21 parse + hierarchy flatten + geom helpers
│  │  ├─ tech.rs           technology file (layer → name/colour/role)
│  │  ├─ geometry.rs       net graph (grid index + Union-Find)
│  │  ├─ transistors.rs    H4 extractor (poly×diffusion → device nets)
│  │  ├─ sim.rs            H5 switch-level 0/1/X solver
│  │  ├─ celllib.rs        H8 netlist → layout generator + cells
│  │  ├─ viewport.rs       polygon → triangulated buffers + hit-test
│  │  └─ tech/default.json embedded default technology
│  ├─ examples/            make_inverter_gds.rs, make_cells.rs
│  ├─ Cargo.toml, tauri.conf.json, build.rs, capabilities/
├─ testdata/              committed synthetic .gds cells
├─ public/, index.html, package.json, vite.config.ts, tsconfig.json
```

## Architecture rules (preserved from the original `workflow.md`)

1. **Rust is the core.** Geometry, simulation, extraction, optimization
   live in Rust. No Python in the production path.
2. **WebGPU is mandatory.** No SVG / DOM / canvas-2D fallback — GDSSIM
   must eventually handle large layouts.
3. **Event-driven simulation is the goal.** Signal changes should
   propagate incrementally rather than recompute the whole chip. (The
   current solver is a full relaxation per evaluation — fine for small
   circuits; the event-driven rebuild is Phase 7.)
4. **Educational first.** Optimise for clarity, visualization,
   interaction — not industrial-scale chips initially.
5. **Build incrementally.** Viewer → geometry → extraction → simulation
   → animation → optimization. Don't skip stages.

## Next

- **H7 — optimization + stateful stepping.** Incremental/event-driven
  sim, GPU batching, and a clocked step/play/reset that holds state.
- **H8b — finish the zoo.** Full adder (a bigger `celllib` netlist) and a
  D flip-flop (needs H7's stateful stepping).

Track progress in [`../workflow.md`](../workflow.md) under the **GDSSIM**
thread.
