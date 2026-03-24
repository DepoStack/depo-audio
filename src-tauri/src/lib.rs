mod analysis;
mod catdetect;
mod commands;
mod dereverb;
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

use types::AppState;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_formats_list,
            commands::detect_format,
            commands::infer_case_name_cmd,
            commands::analyze_audio_cmd,
            commands::score_quality_cmd,
            commands::detect_speakers_cmd,
            commands::available_models_cmd,
            commands::system_capabilities_cmd,
            commands::detect_speech_cmd,
            commands::detect_cat_software_cmd,
            commands::scan_cat_jobs_cmd,
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
