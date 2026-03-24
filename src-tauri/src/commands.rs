use std::fs;
use std::path::Path;

use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use crate::analysis;
use crate::catdetect;
use crate::conversion::do_convert;
use crate::helpers::{detect_format_for_path, get_formats, infer_case_name};
use crate::models;
use crate::scoring;
use crate::speakers;
use crate::persistence::{lib_path, load_library, prefs_path, save_library, save_to_library};
use crate::types::*;

// ── Format commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub fn get_formats_list() -> Vec<FormatInfo> {
    get_formats()
}

#[tauri::command]
pub fn detect_format(path: String) -> Option<FormatInfo> {
    detect_format_for_path(&path)
}

#[tauri::command]
pub fn infer_case_name_cmd(filename: String) -> String {
    infer_case_name(&filename)
}

// ── Analysis command ─────────────────────────────────────────────────────────

#[tauri::command]
pub async fn analyze_audio_cmd(
    app: AppHandle,
    path: String,
) -> Result<crate::types::AnalysisResult, String> {
    analysis::analyze_audio(&app, &path).await
}

// ── Quality scoring command ──────────────────────────────────────────────────

#[tauri::command]
pub async fn score_quality_cmd(
    app: AppHandle,
    path: String,
) -> Result<scoring::QualityScore, String> {
    scoring::score_quality(&app, std::path::Path::new(&path)).await
}

// ── Speaker detection command ───────────────────────────────────────────────

#[tauri::command]
pub async fn detect_speakers_cmd(
    app: AppHandle,
    path: String,
) -> Result<speakers::SpeakerInfo, String> {
    speakers::detect_speakers(&app, std::path::Path::new(&path)).await
}

// ── Model availability command ──────────────────────────────────────────────

#[tauri::command]
pub fn available_models_cmd(app: AppHandle) -> Vec<String> {
    models::available_models(&app)
}

// ── System capabilities command ──────────────────────────────────────────────

#[tauri::command]
pub fn system_capabilities_cmd(app: AppHandle) -> models::SystemCapabilities {
    models::detect_capabilities(&app)
}

// ── CAT software detection commands ──────────────────────────────────────────

#[tauri::command]
pub fn detect_cat_software_cmd() -> Vec<catdetect::CatSoftware> {
    catdetect::detect_cat_software()
}

#[tauri::command]
pub fn scan_cat_jobs_cmd(path: String) -> Vec<catdetect::CatJob> {
    catdetect::scan_cat_jobs(&path)
}

// ── Convert command ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn convert(
    app: AppHandle,
    state: State<'_, AppState>,
    job: ConvertJob,
) -> Result<(), String> {
    let id = job.id.clone();

    let result = do_convert(&app, &job).await;
    match result {
        Ok(files) => {
            save_to_library(&app, &state, &job, &files);
            let _ = app.emit("convert:done", DoneEvent { id, files });
        }
        Err(msg) => {
            let _ = app.emit("convert:error", ErrorEvent { id, message: msg });
        }
    }
    Ok(())
}

// ── Library commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn library_get(app: AppHandle, state: State<'_, AppState>) -> Vec<Case> {
    let lib = load_library(&app);
    *state.library.lock().unwrap() = lib.clone();
    lib.cases
}

#[tauri::command]
pub fn library_rename_case(app: AppHandle, state: State<'_, AppState>, case_id: String, name: String) -> bool {
    let mut lib = state.library.lock().unwrap();
    if let Some(c) = lib.cases.iter_mut().find(|c| c.id == case_id) { c.name = name; }
    save_library(&app, &lib);
    true
}

#[tauri::command]
pub fn library_archive_case(app: AppHandle, state: State<'_, AppState>, case_id: String, archived: bool) -> bool {
    let mut lib = state.library.lock().unwrap();
    if let Some(c) = lib.cases.iter_mut().find(|c| c.id == case_id) { c.archived = archived; }
    save_library(&app, &lib);
    true
}

#[tauri::command]
pub fn library_delete_case(app: AppHandle, state: State<'_, AppState>, case_id: String) -> bool {
    let mut lib = state.library.lock().unwrap();
    lib.cases.retain(|c| c.id != case_id);
    save_library(&app, &lib);
    true
}

#[tauri::command]
pub fn library_delete_session(app: AppHandle, state: State<'_, AppState>, case_id: String, session_id: String) -> bool {
    let mut lib = state.library.lock().unwrap();
    if let Some(c) = lib.cases.iter_mut().find(|c| c.id == case_id) {
        c.sessions.retain(|s| s.id != session_id);
    }
    save_library(&app, &lib);
    true
}

#[tauri::command]
pub fn library_import_file(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    case_name: String,
    label: String,
) -> Result<(), String> {
    let mut lib = state.library.lock().map_err(|e| e.to_string())?;
    let lib_path = lib_path(&app);
    let trimmed = case_name.trim().to_string();
    if trimmed.is_empty() { return Err("Case name cannot be empty".into()); }
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let ext = Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("wav").to_lowercase();
    let source_name = Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or("imported").to_string();
    let idx = lib.cases.iter().position(|c| !c.archived && c.name.to_lowercase() == trimmed.to_lowercase());
    let case = if let Some(i) = idx {
        &mut lib.cases[i]
    } else {
        lib.cases.push(Case { id: Uuid::new_v4().to_string(), name: trimmed.clone(), created_at: Utc::now().to_rfc3339(), archived: false, sessions: vec![] });
        lib.cases.last_mut().unwrap()
    };
    case.sessions.push(Session {
        id: Uuid::new_v4().to_string(),
        date: Utc::now().format("%Y-%m-%d").to_string(),
        source_file: path.clone(),
        source_name,
        participants: vec![Participant { label: label.trim().to_string(), files: vec![LibFile { path, format: ext, size }] }],
    });
    if let Some(parent) = lib_path.parent() { let _ = fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string_pretty(&*lib) { fs::write(&lib_path, json).map_err(|e| e.to_string())?; }
    Ok(())
}

// ── Prefs commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn prefs_get(app: AppHandle, state: State<'_, AppState>) -> Prefs {
    let path = prefs_path(&app);
    let loaded: Prefs = fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    *state.prefs.lock().unwrap() = loaded.clone();
    loaded
}

#[tauri::command]
pub fn prefs_set(app: AppHandle, state: State<'_, AppState>, patch: serde_json::Value) -> bool {
    let mut prefs = state.prefs.lock().unwrap();
    if let Ok(mut current) = serde_json::to_value(prefs.clone()) {
        if let serde_json::Value::Object(ref mut map) = current {
            if let serde_json::Value::Object(patch_map) = patch {
                for (k, v) in patch_map { map.insert(k, v); }
            }
        }
        if let Ok(updated) = serde_json::from_value::<Prefs>(current) {
            *prefs = updated;
        }
    }
    let path = prefs_path(&app);
    if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string_pretty(&*prefs) {
        let _ = fs::write(path, json);
    }
    true
}

// ── Shell / opener commands ───────────────────────────────────────────────────

#[tauri::command]
pub fn show_in_folder(app: AppHandle, path: String) -> Result<(), String> {
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
}
