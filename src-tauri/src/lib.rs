mod analysis;
mod catdetect;
mod commands;
mod dereverb;
mod merge;
mod conversion;
mod denoise;
mod enhance;
mod ffmpeg;
mod helpers;
mod models;
mod persistence;
mod safety;
mod scoring;
mod speakers;
pub mod types;
mod vad;

use tauri::Manager;
use types::AppState;

/// Set ORT_DYLIB_PATH to the bundled ONNX Runtime library so AI features work
/// without requiring users to install onnxruntime separately.
fn setup_onnx_runtime(app: &tauri::AppHandle) {
    if std::env::var("ORT_DYLIB_PATH").is_ok() {
        return; // Already set (e.g. by developer)
    }
    if let Ok(resource_dir) = app.path().resource_dir() {
        let ort_dir = resource_dir.join("resources").join("onnxruntime");
        #[cfg(target_os = "macos")]
        let lib_name = "libonnxruntime.dylib";
        #[cfg(target_os = "windows")]
        let lib_name = "onnxruntime.dll";
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let lib_name = "libonnxruntime.so";

        let lib_path = ort_dir.join(lib_name);
        if lib_path.exists() {
            std::env::set_var("ORT_DYLIB_PATH", &lib_path);
        }
    }
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
            setup_onnx_runtime(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::health_check,
            commands::get_formats_list,
            commands::detect_format,
            commands::infer_case_name_cmd,
            commands::analyze_audio_cmd,
            commands::score_quality_cmd,
            commands::detect_speakers_cmd,
            commands::available_models_cmd,
            commands::system_capabilities_cmd,
            commands::model_catalog_cmd,
            commands::download_model_cmd,
            commands::delete_model_cmd,
            commands::detect_speech_cmd,
            commands::detect_cat_software_cmd,
            commands::scan_cat_jobs_cmd,
            commands::detect_sync_cmd,
            commands::merge_audio_cmd,
            commands::convert,
            commands::show_in_folder,
            commands::library_get,
            commands::library_rename_case,
            commands::library_archive_case,
            commands::library_delete_case,
            commands::library_delete_session,
            commands::library_import_file,
            commands::prefs_get,
            commands::prefs_set,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
