mod analysis;
mod catdetect;
mod commands;
mod conversion;
mod denoise;
mod dereverb;
mod enhance;
mod ffmpeg;
mod helpers;
mod mel;
mod merge;
mod models;
mod persistence;
mod safety;
mod scoring;
mod speakers;
pub mod types;
mod vad;
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

/// Set ORT_DYLIB_PATH to the bundled ONNX Runtime library so AI features work
/// without requiring users to install onnxruntime separately.
///
/// The dylib is pre-flighted with dlopen before ort ever sees it: ort
/// 2.0.0-rc.12 deadlocks (it re-enters its own API OnceLock while building
/// the error) when the dylib fails to load, so a bad library must be caught
/// here — load_session checks the preflight and degrades gracefully.
pub(crate) fn validate_onnx_runtime_api(lib: &libloading::Library) -> Result<String, String> {
    // Loading the DLL/dylib is not enough: OrtGetApiBase can be present while
    // the runtime is too old for the C API requested by the Rust binding.
    unsafe {
        let get_api_base = lib
            .get::<unsafe extern "system" fn() -> *const ort::sys::OrtApiBase>(b"OrtGetApiBase\0")
            .map_err(|e| format!("ONNX Runtime is missing OrtGetApiBase: {e}"))?;
        let base = get_api_base();
        let base = base
            .as_ref()
            .ok_or_else(|| "ONNX Runtime returned a null API base".to_string())?;

        let version_ptr = (base.GetVersionString)();
        let version = if version_ptr.is_null() {
            "unknown".to_string()
        } else {
            std::ffi::CStr::from_ptr(version_ptr).to_string_lossy().into_owned()
        };
        let api = (base.GetApi)(ort::MINOR_VERSION);
        if api.is_null() {
            return Err(format!(
                "ONNX Runtime {version} does not support required C API {}",
                ort::MINOR_VERSION
            ));
        }

        Ok(version)
    }
}

fn setup_onnx_runtime(app: &tauri::AppHandle) {
    // Developer builds may point at a local runtime for model tests. Release
    // builds must never dlopen a caller-controlled environment path; they load
    // only the runtime shipped inside the signed application resources.
    #[cfg(debug_assertions)]
    let preset = std::env::var("ORT_DYLIB_PATH").ok().filter(|s| !s.is_empty());
    #[cfg(not(debug_assertions))]
    let preset: Option<String> = None;
    let lib_path = if let Some(p) = preset {
        std::path::PathBuf::from(p) // explicit developer override
    } else if let Ok(resource_dir) = app.path().resource_dir() {
        #[cfg(target_os = "macos")]
        let lib_path = match resource_dir.parent() {
            Some(contents_dir) => contents_dir.join("Frameworks").join("libonnxruntime.dylib"),
            None => {
                models::set_ort_preflight(Err("Cannot resolve the app Frameworks directory".into()));
                return;
            }
        };
        #[cfg(target_os = "windows")]
        let lib_path = resource_dir
            .join("resources")
            .join("onnxruntime")
            .join("onnxruntime.dll");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let lib_path = resource_dir
            .join("resources")
            .join("onnxruntime")
            .join("libonnxruntime.so");

        // `scripts/setup-dev.sh` intentionally stages ORT under the source
        // tree instead of mutating Tauri's generated dev resource directory.
        // Release builds never take this fallback and therefore continue to
        // load only the library shipped inside the signed application.
        #[cfg(debug_assertions)]
        let lib_path = if lib_path.exists() {
            lib_path
        } else {
            #[cfg(target_os = "macos")]
            let filename = "libonnxruntime.dylib";
            #[cfg(target_os = "windows")]
            let filename = "onnxruntime.dll";
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            let filename = "libonnxruntime.so";
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("resources")
                .join("onnxruntime")
                .join(filename)
        };

        if !lib_path.exists() {
            #[cfg(debug_assertions)]
            let error = "ONNX Runtime library not found in app resources or the local development resources directory";
            #[cfg(not(debug_assertions))]
            let error = "ONNX Runtime library not found in signed app resources";
            models::set_ort_preflight(Err(error.into()));
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
        Ok(lib) => match validate_onnx_runtime_api(&lib) {
            Ok(version) => {
                std::mem::forget(lib); // keep loaded for the process lifetime
                std::env::set_var("ORT_DYLIB_PATH", &lib_path);
                models::set_ort_preflight(Ok(()));
                eprintln!("[ort] Loaded ONNX Runtime {version} with C API {}", ort::MINOR_VERSION);
            }
            Err(e) => {
                eprintln!("[ort] ONNX Runtime is incompatible, AI features disabled: {e}");
                std::env::remove_var("ORT_DYLIB_PATH");
                models::set_ort_preflight(Err(e));
            }
        },
        Err(e) => {
            eprintln!("[ort] ONNX Runtime failed to load, AI features disabled: {e}");
            std::env::remove_var("ORT_DYLIB_PATH");
            models::set_ort_preflight(Err(format!("ONNX Runtime failed to load: {}", e)));
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
            setup_onnx_runtime(app.handle());
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
            commands::score_quality_cmd,
            commands::detect_speakers_cmd,
            commands::system_capabilities_cmd,
            commands::model_catalog_cmd,
            commands::download_model_cmd,
            commands::delete_model_cmd,
            commands::detect_speech_cmd,
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
