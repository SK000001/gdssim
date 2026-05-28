# GDSSIM

Interactive GDS-layout simulator — a subproject of TERN.

Load a `.gds` chip layout → auto-extract the transistor graph → run an
event-driven digital sim → animate signal flow over the real geometry.
Educational-first: a visual transistor explorer and reverse-engineering
tool, not an EDA replacement.

See [Track H in `../roadmap.md`](../roadmap.md) for the full 7-phase
plan; [`../workflow.md`](../workflow.md) for current session state.

## Stack

| Layer       | Tech                          |
| ----------- | ----------------------------- |
| UI          | React + TypeScript            |
| Desktop     | Tauri 2                       |
| Backend     | Rust                          |
| Rendering   | wgpu (WebGPU)                 |
| Parallelism | Rayon (later)                 |

The Tauri webview hosts the React UI. A separate Rust-owned `winit`
window painted by `wgpu` hosts the GPU viewport — Tauri's webview
can't host a `wgpu::Surface` directly, so we keep them as two windows
linked via Tauri IPC. Embedding the GPU viewport inside the webview
is a later milestone.

## Status

**H1 — Project foundation (shipped 2026-05-28).** Tauri + React +
Rust scaffold; `ping` IPC command; `open_viewport` IPC command opens
a wgpu window that renders a clear color + one rectangle drawn in
world coordinates through an orthographic projection. Nothing more.

The rectangle is the seed of the real coordinate system — H2 (GDS
loading) replaces it with parsed polygons.

## Prerequisites

- Rust ≥ 1.85 (Tauri 2 deps require edition 2024). `rustup update stable`.
- Node ≥ 18, pnpm ≥ 9.
- Windows: WebView2 runtime (preinstalled on Windows 11).
- A GPU + driver with Vulkan / DX12 / Metal support.

## Run

```
pnpm install
pnpm tauri dev
```

This starts Vite on http://localhost:1420, compiles the Rust crate,
and launches the Tauri window. Click **Open viewport window** to
spawn the wgpu window.

## Layout

```
gdssim/
├─ src/                  React + TS frontend (Tauri webview)
│  ├─ App.tsx              minimal control panel
│  ├─ App.css
│  ├─ main.tsx
│  └─ vite-env.d.ts
├─ src-tauri/            Rust crate (Tauri backend + GPU viewport)
│  ├─ src/
│  │  ├─ main.rs           thin entry → gdssim_lib::run()
│  │  ├─ lib.rs            Tauri builder + IPC commands (ping, open_viewport)
│  │  ├─ viewport.rs       winit + wgpu window: clear + ortho-rect
│  │  └─ viewport.wgsl     vertex/fragment shader
│  ├─ Cargo.toml
│  ├─ tauri.conf.json
│  ├─ build.rs
│  └─ capabilities/        Tauri permission manifests
├─ public/                static assets served by Vite
├─ index.html             Vite entry HTML
├─ package.json
├─ vite.config.ts
└─ tsconfig.json
```

## Architecture rules (from the original `workflow.md`, preserved here)

1. **Rust is the core.** Geometry, simulation, extraction, optimization
   live in Rust. No Python in the production path.
2. **WebGPU is mandatory.** No SVG / DOM / canvas-only fallback —
   GDSSIM must eventually handle large layouts.
3. **Event-driven simulation only.** Signal changes propagate;
   never recompute the whole chip every frame.
4. **Educational first.** Optimise for clarity, visualization,
   interaction — not industrial-scale chips initially.
5. **Build incrementally.** Viewer → geometry → extraction →
   simulation → animation → optimization. Don't skip stages.

## Next

H2 — GDS loading + viewer: parse `.gds`, render layers, zoom/pan/select.
Track in [`../workflow.md`](../workflow.md) under the **GDSSIM** thread.
