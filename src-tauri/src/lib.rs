//! GDSSIM — Tauri backend entry point.
//!
//! The Tauri webview hosts the React UI (toolbar, panels, inspector).
//! The GPU viewport is a separate Rust-owned `winit` window painted by
//! `wgpu`; see `viewport.rs`. The two are linked via Tauri IPC commands
//! defined below.

mod viewport;

#[tauri::command]
fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
fn open_viewport() -> Result<String, String> {
    viewport::spawn()?;
    Ok("opened".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = env_logger::try_init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![ping, open_viewport])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
