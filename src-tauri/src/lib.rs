//! The Tauri host process: shared state, plugins and the `9drive://` preview protocol.
//!
//! This crate is deliberately thin. Commands validate their input, call `drive-core`, and map
//! the result to `IpcError`; no domain logic and no Google API call lives here.

pub mod protocol;
pub mod state;

use drive_core::db;
use tauri::Manager;
use tauri_plugin_autostart::MacosLauncher;

/// One SQLite file under the app data dir holds every local record.
const DB_FILE: &str = "9drive.db";

/// Autostart launches the app with this argument; the frontend reads it to skip the window and
/// run in the tray only.
const BACKGROUND_ARG: &str = "--background";

pub fn run() {
    tauri::Builder::default()
        // Must be first: a second launch hands its argv over and exits, so nothing else
        // starts twice.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                // `LogDir` resolves the platform's app log dir at run time, which a plugin
                // registered at builder time cannot be handed a path for.
                .target(tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                    file_name: None,
                }))
                .level(tauri_plugin_log::log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![BACKGROUND_ARG]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            app.manage(state::AppState::new(db::open(&dir.join(DB_FILE))?)?);
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol(protocol::SCHEME, protocol::handler)
        .run(tauri::generate_context!())
        .expect("the 9Drive Desktop app starts");
}
