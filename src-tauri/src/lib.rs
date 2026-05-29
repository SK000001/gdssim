//! GDSSIM — Tauri backend.
//!
//! Single-window architecture (H1.5+): the entire UI lives in the
//! React webview, including a WebGPU `<canvas>` viewport. Rust does
//! the heavy lifting (GDS parsing, hierarchy flatten, polygon
//! triangulation, geometry/net processing) and ships ready-to-upload
//! vertex/index buffers back via the `load_gds` command.

mod gds;
mod geometry;
mod tech;
mod viewport;

use std::path::PathBuf;
use std::sync::Mutex;

use gds::Polygon;
use geometry::Nets;

/// The currently-loaded GDS, retained so click hit-testing (H2d) and
/// net queries (H3) can run without re-parsing.
#[derive(Default)]
struct Loaded {
    polys: Vec<Polygon>,
    nets: Nets,
}

#[derive(Default)]
struct LoadStore(Mutex<Loaded>);

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
fn load_gds(path: String, store: tauri::State<LoadStore>) -> Result<viewport::Scene, String> {
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
    let nets = geometry::build_nets(&info.polygons, tech::Tech::default_tech());
    log::info!("built {} nets from {} polygons", nets.count(), info.polygons.len());
    *store.0.lock().unwrap() = Loaded { polys: info.polygons, nets };
    Ok(scene)
}

/// Hit-test a world-space click against the loaded polygons. Returns
/// the smallest containing polygon (with its net id), or `null` on a miss.
#[tauri::command]
fn hit_test(x: f64, y: f64, store: tauri::State<LoadStore>) -> Option<viewport::PolygonHit> {
    let loaded = store.0.lock().unwrap();
    viewport::hit_test(&loaded.polys, &loaded.nets, x, y)
}

/// All polygon rings belonging to a net — lets the frontend highlight a
/// whole electrically-connected net at once.
#[tauri::command]
fn net_rings(net_id: u32, store: tauri::State<LoadStore>) -> Vec<Vec<[f64; 2]>> {
    let loaded = store.0.lock().unwrap();
    let Some(members) = loaded.nets.members.get(net_id as usize) else {
        return Vec::new();
    };
    members
        .iter()
        .filter_map(|&i| loaded.polys.get(i as usize))
        .map(|p| p.points.clone())
        .collect()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = env_logger::try_init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(LoadStore::default())
        .invoke_handler(tauri::generate_handler![ping, load_gds, hit_test, net_rings])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
