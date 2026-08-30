mod analysis;
mod catdetect;
mod commands;
mod conversion;
mod ffmpeg;
mod helpers;
mod merge;
mod models;
mod persistence;
mod safety;
pub mod types;
mod waveform;

use tauri::Manager;
use types::AppState;

/// Re-authorize only the exact audio files recorded in the durable library.
/// This keeps the asset protocol's static scope empty while allowing Library
/// playback after a restart. Asset grants are rebuilt from this source of truth
/// on every launch rather than being persisted independently.
fn allow_library_assets(app: &tauri::AppHandle, library: &types::Library) {
    let scope = app.asset_protocol_scope();
    for file in library
        .cases
        .iter()
        .flat_map(|case| &case.sessions)
        .flat_map(|session| &session.participants)
        .flat_map(|participant| &participant.files)
    {
        let path = std::path::Path::new(&file.path);
        if path.is_file() {
            if let Err(error) = scope.allow_file(path) {
                eprintln!("[scope] Could not authorize a library audio file: {error}");
            }
        }
    }
}

/// Load durable state before any frontend command can mutate it. Corrupt or
/// unreadable files keep their error marker, which makes persistence commands
/// fail closed while still allowing the app to open and report the problem.
fn setup_persistence(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();

    match persistence::load_library(app) {
        Ok(library) => {
            allow_library_assets(app, &library);
            *state.library.lock().unwrap_or_else(|e| e.into_inner()) = library;
            *state.library_load_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        Err(error) => {
            eprintln!("[persistence] Library load failed; writes disabled: {error}");
            *state.library_load_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
        }
    }

    match persistence::load_prefs(app) {
        Ok(prefs) => {
            *state.prefs.lock().unwrap_or_else(|e| e.into_inner()) = prefs;
            *state.prefs_load_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
        Err(error) => {
            eprintln!("[persistence] Preferences load failed; writes disabled: {error}");
            *state.prefs_load_error.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
        }
    }
}

#[cfg(desktop)]
fn updater_config_is_valid(config: Option<&serde_json::Value>) -> bool {
    config
        .and_then(|value| serde_json::from_value::<tauri_plugin_updater::Config>(value.clone()).ok())
        .is_some_and(|config| !config.endpoints.is_empty() && !config.pubkey.trim().is_empty())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .setup(|app| {
            setup_persistence(app.handle());
            // Auto-update is available only when the release overlay contains
            // a complete signed-updater configuration. Tauri exposes a missing
            // plugin configuration as JSON null; registering the updater in
            // that state aborts app setup before the first window can open.
            #[cfg(desktop)]
            {
                let updater_config = app.config().plugins.0.get("updater");
                if updater_config_is_valid(updater_config) {
                    app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
                    app.handle().plugin(tauri_plugin_process::init())?;
                } else if updater_config.is_some() {
                    eprintln!("[updater] Invalid updater configuration; signed in-app updates are disabled");
                } else {
                    eprintln!("[updater] No signed updater configuration; signed in-app updates are disabled");
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::health_check,
            commands::get_formats_list,
            commands::detect_format,
            commands::infer_case_name_cmd,
            commands::analyze_audio_cmd,
            commands::cancel_scan_cmd,
            commands::system_capabilities_cmd,
            commands::legacy_model_cleanup_catalog_cmd,
            commands::delete_legacy_model_cmd,
            commands::waveform_peaks_cmd,
            commands::cancel_waveform_cmd,
            commands::detect_cat_software_cmd,
            commands::scan_cat_jobs_cmd,
            commands::detect_sync_cmd,
            commands::merge_audio_cmd,
            commands::begin_conversion_batch_cmd,
            commands::cancel_conversion_cmd,
            commands::convert,
            commands::show_in_folder,
            commands::library_get,
            commands::library_rename_case,
            commands::library_archive_case,
            commands::library_delete_case,
            commands::library_delete_session,
            commands::library_import_file,
            commands::library_import_files,
            commands::prefs_get,
            commands::prefs_set,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(all(test, desktop))]
mod updater_config_tests {
    use super::updater_config_is_valid;

    #[test]
    fn updater_requires_a_complete_plugin_config() {
        assert!(!updater_config_is_valid(None));
        assert!(!updater_config_is_valid(Some(&serde_json::Value::Null)));
        assert!(!updater_config_is_valid(Some(&serde_json::json!({}))));
        assert!(!updater_config_is_valid(Some(&serde_json::json!({
            "endpoints": [],
            "pubkey": "test-key"
        }))));
        assert!(!updater_config_is_valid(Some(&serde_json::json!({
            "endpoints": ["https://github.com/DepoStack/depo-audio/releases/latest/download/latest.json"],
            "pubkey": "  "
        }))));
        assert!(!updater_config_is_valid(Some(&serde_json::json!({
            "endpoints": ["not-a-url"],
            "pubkey": "test-key"
        }))));
        assert!(updater_config_is_valid(Some(&serde_json::json!({
            "endpoints": ["https://github.com/DepoStack/depo-audio/releases/latest/download/latest.json"],
            "pubkey": "test-key"
        }))));
    }
}
