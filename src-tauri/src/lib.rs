//! The Tauri host process: shared state, plugins and the `9drive://` preview protocol.
//!
//! This crate is deliberately thin. Commands validate their input, call `drive-core`, and map
//! the result to `IpcError`; no domain logic and no Google API call lives here.

pub mod commands;
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
        .invoke_handler(tauri::generate_handler![
            // accounts / app
            commands::accounts::app_info,
            commands::accounts::open_app_dir,
            commands::accounts::list_accounts,
            commands::accounts::begin_google_auth,
            commands::accounts::complete_google_auth,
            commands::accounts::disconnect_account,
            commands::accounts::sync_account_quota,
            commands::accounts::refresh_quota,
            // folders — the virtual tree is local SQLite only, it never touches Drive
            commands::folders::list_folders,
            commands::folders::create_folder,
            commands::folders::rename_folder,
            commands::folders::move_folder,
            commands::folders::delete_folder,
            commands::folders::recent_folders,
            // files
            commands::files::list_files,
            commands::files::search_files,
            commands::files::get_file,
            commands::files::set_starred,
            commands::files::set_archived,
            commands::files::rename_file,
            commands::files::batch_update,
            commands::files::move_files,
            commands::files::delete_files,
            commands::files::trash_files,
            commands::files::untrash_files,
            commands::files::toggle_star,
            commands::files::toggle_archive,
            commands::files::preview_data,
            commands::files::save_file_as,
            // sync
            commands::accounts::sync_google,
            // upload
            commands::uploads::start_upload_batch,
            commands::uploads::cancel_upload,
            commands::uploads::retry_upload,
            // settings
            commands::settings::get_settings,
            commands::settings::set_settings,
            commands::settings::set_google_client_id,
        ])
        .register_asynchronous_uri_scheme_protocol(protocol::SCHEME, protocol::handler)
        .run(tauri::generate_context!())
        .expect("the 9Drive Desktop app starts");
}
