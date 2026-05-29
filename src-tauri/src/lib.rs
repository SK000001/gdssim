//! GDSSIM — Tauri backend.
//!
//! Single-window architecture (H1.5+): the entire UI lives in the
//! React webview, including a WebGPU `<canvas>` viewport. Rust does
//! the heavy lifting (GDS parsing, hierarchy flatten, polygon
//! triangulation) and ships ready-to-upload vertex/index buffers
//! back via the `load_gds` command.

mod gds;
mod tech;
mod viewport;

use std::path::PathBuf;
use std::sync::Mutex;

use gds::Polygon;

/// The flattened polygon list of the currently-loaded GDS, retained so
/// click hit-testing (H2d) can run against it without re-parsing.
#[derive(Default)]
struct PolyStore(Mutex<Vec<Polygon>>);

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
fn load_gds(path: String, store: tauri::State<PolyStore>) -> Result<viewport::Scene, String> {
    let path = PathBuf::from(path);
    let info = std::panic::catch_unwind(|| gds::load_and_flatten(&path))
        .map_err(|p| {
            let msg = p
                .downcast_ref::<&'static str>()
                .map(|s| (*s).to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic in GDS parser".into());
            format!("parser panic: {msg}")
        })?
        .map_err(|e| e.to_string())?;
    let scene = viewport::build_scene(&info);
    *store.0.lock().unwrap() = info.polygons;
    Ok(scene)
}

/// Hit-test a world-space click against the loaded polygons. Returns
/// the smallest containing polygon, or `null` when the click misses.
#[tauri::command]
fn hit_test(x: f64, y: f64, store: tauri::State<PolyStore>) -> Option<viewport::PolygonHit> {
    let polys = store.0.lock().unwrap();
    viewport::hit_test(&polys, x, y)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = env_logger::try_init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(PolyStore::default())
        .invoke_handler(tauri::generate_handler![ping, load_gds, hit_test])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
