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
    viewport::show()?;
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
    // gds21 reads BGNLIB / BGNSTR date stamps via chrono, which can
    // panic on out-of-range / garbage dates that real-world GDS files
    // sometimes carry. The Tauri command runs on a webview COM
    // callback path, so a bare panic crosses an FFI boundary and
    // aborts the process. Trap it.
    let path = PathBuf::from(path);
    let polys = std::panic::catch_unwind(|| gds::load_and_flatten(&path))
        .map_err(|p| {
            let msg = p
                .downcast_ref::<&'static str>()
                .map(|s| (*s).to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic in GDS parser (likely a malformed date in the file header)".into());
            format!("parser panic: {msg}")
        })?
        .map_err(|e| e.to_string())?;

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
    // Make sure the viewport is visible before we hand it the scene.
    viewport::show()?;
    viewport::send_scene(polys)?;
    Ok(summary)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = env_logger::try_init();
    // Bring the viewport thread up before Tauri starts and show its
    // window immediately — the app is the viewport; the webview is
    // just a control strip. Closing the viewport doesn't tear the
    // loop down, so `load_gds` can still reopen it on demand.
    if let Err(e) = viewport::init() {
        log::error!("viewport init failed: {e}");
    } else if let Err(e) = viewport::show() {
        log::error!("viewport show failed: {e}");
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![ping, open_viewport, load_gds])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
