//! GDSSIM — Tauri backend entry point.
//!
//! The Tauri webview hosts the React UI. The GPU viewport is a separate
//! Rust-owned `winit` window painted by `wgpu`; see `viewport.rs`. GDS
//! files are parsed by `gds.rs` and pushed into the viewport via the
//! `EventLoopProxy` stashed in `viewport::VIEWPORT_PROXY`.

mod gds;
mod viewport;

use std::path::PathBuf;

use serde::Serialize;

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
fn open_viewport() -> Result<String, String> {
    viewport::spawn()?;
    Ok("opened".into())
}

#[derive(Debug, Serialize)]
struct LoadGdsResult {
    polygon_count: usize,
    layers: Vec<i16>,
    bbox_min: [f64; 2],
    bbox_max: [f64; 2],
}

#[tauri::command]
fn load_gds(path: String) -> Result<LoadGdsResult, String> {
    let path = PathBuf::from(path);
    let polys = gds::load_and_flatten(&path).map_err(|e| e.to_string())?;
    let bbox = gds::bbox(&polys);
    let mut layers: Vec<i16> = polys.iter().map(|p| p.layer).collect();
    layers.sort_unstable();
    layers.dedup();
    let summary = LoadGdsResult {
        polygon_count: polys.len(),
        layers,
        bbox_min: bbox.min,
        bbox_max: bbox.max,
    };
    viewport::send_scene(polys)?;
    Ok(summary)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = env_logger::try_init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![ping, open_viewport, load_gds])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
