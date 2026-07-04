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
mod mel;
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
///
/// The dylib is pre-flighted with dlopen before ort ever sees it: ort
/// 2.0.0-rc.12 deadlocks (it re-enters its own API OnceLock while building
/// the error) when the dylib fails to load, so a bad library must be caught
/// here — load_session checks the preflight and degrades gracefully.
fn setup_onnx_runtime(app: &tauri::AppHandle) {
    let preset = std::env::var("ORT_DYLIB_PATH").ok().filter(|s| !s.is_empty());
    let lib_path = if let Some(p) = preset {
        std::path::PathBuf::from(p) // already set (e.g. by developer)
    } else if let Ok(resource_dir) = app.path().resource_dir() {
        let ort_dir = resource_dir.join("resources").join("onnxruntime");
        #[cfg(target_os = "macos")]
        let lib_name = "libonnxruntime.dylib";
        #[cfg(target_os = "windows")]
        let lib_name = "onnxruntime.dll";
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let lib_name = "libonnxruntime.so";

        let lib_path = ort_dir.join(lib_name);
        if !lib_path.exists() {
            models::set_ort_preflight(Err("ONNX Runtime library not found in app resources".into()));
            return;
        }
        lib_path
    } else {
        models::set_ort_preflight(Err("Cannot resolve app resource directory".into()));
        return;
    };

    // dlopen exactly what ort will dlopen. Keeping the handle alive means
    // ort's own load of the same path reuses it (no second TLS allocation).
    match unsafe { libloading::Library::new(&lib_path) } {
        Ok(lib) => {
            std::mem::forget(lib); // keep loaded for the process lifetime
            std::env::set_var("ORT_DYLIB_PATH", &lib_path);
            models::set_ort_preflight(Ok(()));
        }
        Err(e) => {
            eprintln!("[ort] ONNX Runtime failed to load, AI features disabled: {}", e);
            std::env::remove_var("ORT_DYLIB_PATH");
            models::set_ort_preflight(Err(format!("ONNX Runtime failed to load: {}", e)));
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
        // Closing the window quits immediately. A long-running analysis or
        // conversion (synchronous ONNX inference + ffmpeg children) can block a
        // graceful async shutdown, leaving the app seemingly stuck; exiting the
        // process directly guarantees "close means close". Completed work
        // (library entries, converted files) is already persisted to disk.
        .on_window_event(|_window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                std::process::exit(0);
            }
        })
        .manage(AppState::default())
        .setup(|app| {
            setup_onnx_runtime(app.handle());
            // Auto-update via GitHub Releases (desktop only)
            #[cfg(desktop)]
            {
                app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
                app.handle().plugin(tauri_plugin_process::init())?;
            }
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
