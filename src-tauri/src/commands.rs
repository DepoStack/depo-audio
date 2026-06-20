use std::fs;
use std::path::Path;

use chrono::Utc;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_shell::ShellExt;
use uuid::Uuid;

use crate::analysis;
use crate::catdetect;
use crate::conversion::do_convert;
use crate::merge;
use crate::vad;
use crate::helpers::{detect_format_for_path, get_formats, infer_case_name};
use crate::models;
use crate::scoring;
use crate::speakers;
use crate::persistence::{load_library, prefs_path, save_library, save_to_library};
use crate::types::*;

// ── Health check ─────────────────────────────────────────────────────────────

async fn sidecar_runs(app: &AppHandle, bin: &str) -> bool {
    match app.shell().sidecar(bin) {
        Ok(cmd) => matches!(cmd.args(["-version"]).output().await, Ok(out) if out.status.success()),
        Err(_) => false,
    }
}

#[tauri::command]
pub async fn health_check(app: AppHandle) -> Result<serde_json::Value, String> {
    // Actually execute the sidecars — constructing the command alone does not
    // verify the binary exists or runs.
    let ffmpeg_ok = sidecar_runs(&app, crate::helpers::ffmpeg_bin_name()).await;
    let ffprobe_ok = sidecar_runs(&app, crate::helpers::ffprobe_bin_name()).await;

    let models = models::available_models(&app);
    let caps = models::detect_capabilities(&app);

    Ok(serde_json::json!({
        "ffmpeg": ffmpeg_ok,
        "ffprobe": ffprobe_ok,
        "models": models,
        "accelerator": caps.accelerator,
        "tier": caps.tier,
    }))
}

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

// ── System capabilities command ──────────────────────────────────────────────

#[tauri::command]
pub fn system_capabilities_cmd(app: AppHandle) -> models::SystemCapabilities {
    models::detect_capabilities(&app)
}

// ── Model management commands ──────────────────────────────────────────────

#[tauri::command]
pub fn model_catalog_cmd(app: AppHandle) -> Vec<models::ModelInfo> {
    models::model_catalog(&app)
}

#[tauri::command]
pub async fn download_model_cmd(app: AppHandle, filename: String) -> Result<String, String> {
    models::download_model(&app, &filename).await
}

#[tauri::command]
pub fn delete_model_cmd(app: AppHandle, filename: String) -> Result<(), String> {
    models::delete_model(&app, &filename)
}

// ── VAD command ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn detect_speech_cmd(
    app: AppHandle,
    path: String,
) -> Result<vad::VadResult, String> {
    vad::detect_speech(&app, std::path::Path::new(&path)).await
}

// ── CAT software detection commands ──────────────────────────────────────────

#[tauri::command]
pub fn detect_cat_software_cmd(max_depth: Option<u32>) -> Vec<catdetect::CatSoftware> {
    // Honor the user's "Folder Scan Depth" setting, bounded to sane limits
    let depth = max_depth.unwrap_or(5).clamp(1, 20) as usize;
    catdetect::detect_cat_software(depth)
}

#[tauri::command]
pub fn scan_cat_jobs_cmd(path: String) -> Vec<catdetect::CatJob> {
    catdetect::scan_cat_jobs(&path)
}

// ── Merge commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn detect_sync_cmd(
    app: AppHandle,
    source_a: String,
    source_b: String,
) -> Result<merge::SyncResult, String> {
    merge::detect_sync(&app, &source_a, &source_b).await
}

#[tauri::command]
pub async fn merge_audio_cmd(
    app: AppHandle,
    job: merge::MergeJob,
) -> Result<merge::MergeResult, String> {
    merge::merge_audio(&app, &job).await
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
    let mut lib = state.library.lock().unwrap_or_else(|e| e.into_inner());
    // Load from disk only if in-memory state is empty (first call)
    if lib.cases.is_empty() {
        let loaded = load_library(&app);
        *lib = loaded;
    }
    lib.cases.clone()
}

#[tauri::command]
pub fn library_rename_case(app: AppHandle, state: State<'_, AppState>, case_id: String, name: String) -> bool {
    let mut lib = state.library.lock().unwrap_or_else(|e| e.into_inner());
    let found = if let Some(c) = lib.cases.iter_mut().find(|c| c.id == case_id) { c.name = name; true } else { false };
    found && save_library(&app, &lib).is_ok()
}

#[tauri::command]
pub fn library_archive_case(app: AppHandle, state: State<'_, AppState>, case_id: String, archived: bool) -> bool {
    let mut lib = state.library.lock().unwrap_or_else(|e| e.into_inner());
    let found = if let Some(c) = lib.cases.iter_mut().find(|c| c.id == case_id) { c.archived = archived; true } else { false };
    found && save_library(&app, &lib).is_ok()
}

#[tauri::command]
pub fn library_delete_case(app: AppHandle, state: State<'_, AppState>, case_id: String) -> bool {
    let mut lib = state.library.lock().unwrap_or_else(|e| e.into_inner());
    let before = lib.cases.len();
    lib.cases.retain(|c| c.id != case_id);
    let removed = lib.cases.len() != before;
    removed && save_library(&app, &lib).is_ok()
}

#[tauri::command]
pub fn library_delete_session(app: AppHandle, state: State<'_, AppState>, case_id: String, session_id: String) -> bool {
    let mut lib = state.library.lock().unwrap_or_else(|e| e.into_inner());
    let removed = if let Some(c) = lib.cases.iter_mut().find(|c| c.id == case_id) {
        let before = c.sessions.len();
        c.sessions.retain(|s| s.id != session_id);
        c.sessions.len() != before
    } else { false };
    removed && save_library(&app, &lib).is_ok()
}

#[tauri::command]
pub fn library_import_file(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    case_name: String,
    label: String,
) -> Result<(), String> {
    // Validate inputs
    crate::safety::check_file_safe(std::path::Path::new(&path))?;
    let trimmed = case_name.trim().to_string();
    if trimmed.is_empty() { return Err("Case name cannot be empty".into()); }
    if trimmed.len() > 200 { return Err("Case name is too long (max 200 characters)".into()); }
    let label_trimmed = label.trim().to_string();
    if label_trimmed.len() > 100 { return Err("Label is too long (max 100 characters)".into()); }
    // Sanitize case name: remove path separators and control characters
    let sanitized_name: String = trimmed.chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\' && *c != ':')
        .collect();
    if sanitized_name.is_empty() { return Err("Case name contains only invalid characters".into()); }

    let mut lib = state.library.lock().unwrap_or_else(|e| e.into_inner());
    let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let ext = Path::new(&path).extension().and_then(|e| e.to_str()).unwrap_or("wav").to_lowercase();
    let source_name = Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or("imported").to_string();
    let idx = crate::persistence::find_case_idx(&lib.cases, &sanitized_name);
    let case = if let Some(i) = idx {
        // Importing into an existing case re-activates it if archived — the
        // user is explicitly adding content, so it should be visible again.
        lib.cases[i].archived = false;
        &mut lib.cases[i]
    } else {
        lib.cases.push(Case { id: Uuid::new_v4().to_string(), name: sanitized_name.clone(), created_at: Utc::now().to_rfc3339(), archived: false, sessions: vec![] });
        lib.cases.last_mut().unwrap()
    };
    case.sessions.push(Session {
        id: Uuid::new_v4().to_string(),
        date: Utc::now().format("%Y-%m-%d").to_string(),
        source_file: path.clone(),
        source_name,
        participants: vec![Participant { label: label_trimmed.clone(), files: vec![LibFile { path, format: ext, size }] }],
    });
    save_library(&app, &lib)?;
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
    *state.prefs.lock().unwrap_or_else(|e| e.into_inner()) = loaded.clone();
    loaded
}

#[tauri::command]
pub fn prefs_set(app: AppHandle, state: State<'_, AppState>, patch: serde_json::Value) -> bool {
    let mut prefs = state.prefs.lock().unwrap_or_else(|e| e.into_inner());
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
    match serde_json::to_string_pretty(&*prefs) {
        Ok(json) => crate::persistence::atomic_write(&path, json.as_bytes()).is_ok(),
        Err(_) => false,
    }
}

// ── Shell / opener commands ───────────────────────────────────────────────────

#[tauri::command]
pub fn show_in_folder(app: AppHandle, path: String) -> Result<(), String> {
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| e.to_string())
}
