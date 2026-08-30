use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;
use uuid::Uuid;

use crate::analysis;
use crate::catdetect;
use crate::conversion::{do_convert, ConversionCancel};
use crate::helpers::{detect_format_for_path, get_formats, infer_case_name};
use crate::merge;
use crate::models;
use crate::persistence::{library_ready, mutate_library, prefs_path, prefs_ready, save_to_library};
use crate::types::*;

// ── Health check ─────────────────────────────────────────────────────────────

async fn sidecar_runs(app: &AppHandle, bin: &str) -> bool {
    matches!(
        crate::ffmpeg::sidecar_output_opt(app, bin, vec!["-version".into()], 10).await,
        Some(output) if output.success
    )
}

#[tauri::command]
pub async fn health_check(app: AppHandle) -> Result<serde_json::Value, String> {
    // Actually execute the sidecars — constructing the command alone does not
    // verify the binary exists or runs.
    let ffmpeg_ok = sidecar_runs(&app, crate::helpers::ffmpeg_bin_name()).await;
    let ffprobe_ok = sidecar_runs(&app, crate::helpers::ffprobe_bin_name()).await;
    let ftr_decoder = if ffmpeg_ok {
        crate::ffmpeg::ftr_decoder_available(&app).await.unwrap_or(false)
    } else {
        false
    };

    Ok(serde_json::json!({
        "ffmpeg": ffmpeg_ok,
        "ffprobe": ffprobe_ok,
        "ftrDecoder": ftr_decoder,
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
    state: State<'_, AppState>,
    path: String,
) -> Result<crate::types::AnalysisResult, String> {
    // ScanCtx gives the frontend within-file progress events and makes this
    // scan cancellable via cancel_scan_cmd.
    let ctx = analysis::ScanCtx::new(app.clone(), path.clone(), state.scan_epoch.clone());
    analysis::analyze_audio(&app, &path, Some(&ctx)).await
}

/// Cancel all in-flight scans: bumps the scan epoch, which every running
/// analysis (including its blocking inference tasks) checks between steps.
#[tauri::command]
pub fn cancel_scan_cmd(state: State<'_, AppState>) {
    state.scan_epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

// ── System capabilities command ──────────────────────────────────────────────

#[tauri::command]
pub fn system_capabilities_cmd() -> models::SystemCapabilities {
    models::detect_capabilities()
}

// ── Model management commands ──────────────────────────────────────────────

#[tauri::command]
pub fn legacy_model_cleanup_catalog_cmd(app: AppHandle) -> Vec<models::LegacyModelInfo> {
    models::legacy_model_cleanup_catalog(&app)
}

#[tauri::command]
pub fn delete_legacy_model_cmd(app: AppHandle, filename: String) -> Result<(), String> {
    models::delete_legacy_model(&app, &filename)
}

// ── VAD command ─────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn waveform_peaks_cmd(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    request_id: String,
) -> Result<Vec<crate::waveform::WaveformPeak>, String> {
    crate::waveform::request_waveform(&app, &state.waveform, path, request_id).await
}

#[tauri::command]
pub fn cancel_waveform_cmd(state: State<'_, AppState>, request_id: String) -> Result<bool, String> {
    state.waveform.cancel(&request_id)
}

// ── CAT software detection commands ──────────────────────────────────────────

#[tauri::command]
pub async fn detect_cat_software_cmd(max_depth: Option<u32>) -> Result<Vec<catdetect::CatSoftware>, String> {
    // Honor the user's "Folder Scan Depth" setting, bounded to sane limits
    let depth = max_depth.unwrap_or(catdetect::DEFAULT_SCAN_DEPTH as u32).clamp(1, 20) as usize;
    tauri::async_runtime::spawn_blocking(move || catdetect::detect_cat_software(depth))
        .await
        .map_err(|error| format!("CAT software scan failed: {error}"))
}

#[tauri::command]
pub async fn scan_cat_jobs_cmd(path: String, max_depth: Option<u32>) -> Result<Vec<catdetect::CatJob>, String> {
    let depth = max_depth.unwrap_or(catdetect::DEFAULT_SCAN_DEPTH as u32).clamp(1, 20) as usize;
    tauri::async_runtime::spawn_blocking(move || catdetect::scan_cat_jobs(&path, depth))
        .await
        .map_err(|error| format!("CAT job scan failed: {error}"))
}

// ── Merge commands ───────────────────────────────────────────────────────────

#[tauri::command]
pub async fn detect_sync_cmd(app: AppHandle, source_a: String, source_b: String) -> Result<merge::SyncResult, String> {
    merge::detect_sync(&app, &source_a, &source_b).await
}

#[tauri::command]
pub async fn merge_audio_cmd(app: AppHandle, job: merge::MergeJob) -> Result<merge::MergeResult, String> {
    let mut result = merge::merge_audio(&app, &job).await?;
    if let Err(error) = app.asset_protocol_scope().allow_file(&result.output_path) {
        result.warning = Some(format!(
            "Merge finished, but its output could not be authorized for playback. Restart DepoAudio to restore Library playback access: {error}"
        ));
    }
    Ok(result)
}

// ── Convert command ───────────────────────────────────────────────────────────

fn advance_conversion_epoch(epoch: &AtomicU64) -> u64 {
    epoch.fetch_add(1, Ordering::AcqRel).wrapping_add(1)
}

#[tauri::command]
pub fn begin_conversion_batch_cmd(state: State<'_, AppState>) -> u64 {
    let _commit = state.conversion_commit.lock().unwrap_or_else(|e| e.into_inner());
    advance_conversion_epoch(&state.conversion_epoch)
}

#[tauri::command]
pub fn cancel_conversion_cmd(state: State<'_, AppState>) -> u64 {
    let _commit = state.conversion_commit.lock().unwrap_or_else(|e| e.into_inner());
    advance_conversion_epoch(&state.conversion_epoch)
}

fn emit_conversion_cancelled(app: &AppHandle, id: String) -> Result<(), String> {
    app.emit(
        "convert:cancelled",
        ErrorEvent {
            id,
            message: CONVERSION_CANCELLED_MESSAGE.into(),
        },
    )
    .map_err(|error| format!("Conversion was cancelled, but cancellation could not be reported: {error}"))
}

#[tauri::command]
pub async fn convert(app: AppHandle, state: State<'_, AppState>, job: ConvertJob) -> Result<(), String> {
    let id = job.id.clone();
    let cancel = ConversionCancel::new(state.conversion_epoch.clone(), job.cancel_generation);

    let result = do_convert(&app, &job, &cancel).await;
    match result {
        Ok(files) => {
            // Serialize this last generation check and every externally visible
            // commit with begin/cancel. If cancel returns first, this check sees
            // the new epoch; if this guard wins first, the conversion is fully
            // committed before cancel can return.
            let _commit = state.conversion_commit.lock().unwrap_or_else(|e| e.into_inner());
            let _asset_commit = state.library_asset_commit.lock().unwrap_or_else(|e| e.into_inner());
            // Honor a cancel racing the final encoder event before committing
            // anything to the persistent library.
            if cancel.cancelled() {
                for file in &files {
                    let _ = fs::remove_file(&file.path);
                }
                return emit_conversion_cancelled(&app, id);
            }

            let mut warning = save_to_library(&app, &state, &job, &files)
                .err()
                .map(|error| format!("The audio was converted, but the library could not be updated: {error}"));
            let library_warning = warning.is_some();

            let scope = app.asset_protocol_scope();
            let mut scope_failures = 0usize;
            let mut first_scope_error = None;
            for file in &files {
                if let Err(error) = scope.allow_file(&file.path) {
                    scope_failures += 1;
                    first_scope_error.get_or_insert_with(|| format!("{}: {error}", file.name));
                }
            }
            if scope_failures > 0 {
                let error = format!(
                    "Conversion finished, but {scope_failures} output file(s) could not be authorized for playback. Restart DepoAudio to rebuild access from the Library. First error: {}",
                    first_scope_error.unwrap_or_else(|| "unknown asset-scope error".into())
                );
                warning = Some(match warning {
                    Some(existing) => format!("{existing} {error}"),
                    None => error,
                });
            }
            app.emit(
                "convert:done",
                DoneEvent {
                    id,
                    files,
                    warning,
                    library_warning,
                },
            )
            .map_err(|error| format!("Conversion finished, but completion could not be reported: {error}"))?;
        }
        Err(msg) if msg == CONVERSION_CANCELLED_MESSAGE => emit_conversion_cancelled(&app, id)?,
        Err(msg) => {
            let invoke_error = msg.clone();
            if app.emit("convert:error", ErrorEvent { id, message: msg }).is_err() {
                return Err(invoke_error);
            }
        }
    }
    Ok(())
}

// ── Library commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn library_get(_app: AppHandle, state: State<'_, AppState>) -> Result<Vec<Case>, String> {
    library_ready(&state)?;
    let lib = state.library.lock().unwrap_or_else(|e| e.into_inner());
    Ok(lib.cases.clone())
}

#[tauri::command]
pub fn library_rename_case(
    app: AppHandle,
    state: State<'_, AppState>,
    case_id: String,
    name: String,
) -> Result<bool, String> {
    let name = crate::helpers::sanitize_case_name(&name)?;
    mutate_library(&app, &state, move |lib| {
        Ok(
            if let Some(case) = lib.cases.iter_mut().find(|case| case.id == case_id) {
                case.name = name;
                true
            } else {
                false
            },
        )
    })
}

#[tauri::command]
pub fn library_archive_case(
    app: AppHandle,
    state: State<'_, AppState>,
    case_id: String,
    archived: bool,
) -> Result<bool, String> {
    mutate_library(&app, &state, move |lib| {
        Ok(
            if let Some(case) = lib.cases.iter_mut().find(|case| case.id == case_id) {
                case.archived = archived;
                true
            } else {
                false
            },
        )
    })
}

#[tauri::command]
pub fn library_delete_case(
    app: AppHandle,
    state: State<'_, AppState>,
    case_id: String,
) -> Result<LibraryMutationResult, String> {
    let _asset_commit = state.library_asset_commit.lock().unwrap_or_else(|e| e.into_inner());
    let (removed, paths) = mutate_library(&app, &state, move |lib| {
        let Some(index) = lib.cases.iter().position(|case| case.id == case_id) else {
            return Ok((false, Vec::new()));
        };
        let removed_case = lib.cases.remove(index);
        let candidates = case_paths(&removed_case);
        Ok((true, unreferenced_paths(lib, candidates)))
    })?;
    Ok(LibraryMutationResult {
        changed: removed,
        warning: revoke_library_assets(&app, &paths),
    })
}

#[tauri::command]
pub fn library_delete_session(
    app: AppHandle,
    state: State<'_, AppState>,
    case_id: String,
    session_id: String,
) -> Result<LibraryMutationResult, String> {
    let _asset_commit = state.library_asset_commit.lock().unwrap_or_else(|e| e.into_inner());
    let (removed, paths) = mutate_library(&app, &state, move |lib| {
        let removed_session = {
            let Some(case) = lib.cases.iter_mut().find(|case| case.id == case_id) else {
                return Ok((false, Vec::new()));
            };
            let Some(index) = case.sessions.iter().position(|session| session.id == session_id) else {
                return Ok((false, Vec::new()));
            };
            case.sessions.remove(index)
        };
        let candidates = session_paths(&removed_session);
        Ok((true, unreferenced_paths(lib, candidates)))
    })?;
    Ok(LibraryMutationResult {
        changed: removed,
        warning: revoke_library_assets(&app, &paths),
    })
}

fn session_paths(session: &Session) -> Vec<String> {
    session
        .participants
        .iter()
        .flat_map(|participant| &participant.files)
        .map(|file| file.path.clone())
        .collect()
}

fn case_paths(case: &Case) -> Vec<String> {
    case.sessions.iter().flat_map(session_paths).collect()
}

fn unreferenced_paths(library: &Library, candidates: Vec<String>) -> Vec<String> {
    let referenced: HashSet<&str> = library
        .cases
        .iter()
        .flat_map(|case| &case.sessions)
        .flat_map(|session| &session.participants)
        .flat_map(|participant| &participant.files)
        .map(|file| file.path.as_str())
        .collect();
    let mut unique = HashSet::new();
    candidates
        .into_iter()
        .filter(|path| !referenced.contains(path.as_str()) && unique.insert(path.clone()))
        .collect()
}

fn revoke_library_assets(app: &AppHandle, paths: &[String]) -> Option<String> {
    let scope = app.asset_protocol_scope();
    let mut failures = 0usize;
    let mut first_error = None;
    for path in paths {
        if let Err(error) = scope.forbid_file(path) {
            failures += 1;
            first_error.get_or_insert_with(|| error.to_string());
        }
    }
    (failures > 0).then(|| {
        format!(
            "The Library entry was removed, but playback access could not be revoked for {failures} file(s). Restart DepoAudio to reset access. First error: {}",
            first_error.unwrap_or_else(|| "unknown asset-scope error".into())
        )
    })
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryMutationResult {
    pub changed: bool,
    pub warning: Option<String>,
}

fn playable_library_extension(path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "Library imports must have a supported audio extension".to_string())?;
    if matches!(
        extension.as_str(),
        "wav" | "mp3" | "flac" | "aac" | "ogg" | "opus" | "wma" | "m4a" | "aif" | "aiff"
    ) {
        Ok(extension)
    } else {
        Err(format!("Unsupported Library audio extension: {extension}"))
    }
}

#[tauri::command]
pub fn library_import_file(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
    case_name: String,
    label: String,
) -> Result<LibraryMutationResult, String> {
    library_import_files_inner(&app, &state, vec![path], case_name, label)
}

/// Import a picker selection as one persistence transaction. Every source is
/// validated before the candidate library is mutated, and mutate_library only
/// publishes the candidate after its atomic disk write succeeds.
#[tauri::command]
pub fn library_import_files(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    case_name: String,
    label: String,
) -> Result<LibraryMutationResult, String> {
    library_import_files_inner(&app, &state, paths, case_name, label)
}

fn library_import_files_inner(
    app: &AppHandle,
    state: &State<'_, AppState>,
    paths: Vec<String>,
    case_name: String,
    label: String,
) -> Result<LibraryMutationResult, String> {
    let _asset_commit = state.library_asset_commit.lock().unwrap_or_else(|e| e.into_inner());
    if paths.is_empty() {
        return Err("Select at least one file".into());
    }
    if paths.len() > 1_000 {
        return Err("Too many files selected (maximum 1000)".into());
    }
    let sanitized_name = crate::helpers::sanitize_case_name(&case_name)?;
    let label_trimmed = label.trim().to_string();
    if label_trimmed.is_empty() {
        return Err("Label cannot be empty".into());
    }
    if label_trimmed.len() > 100 {
        return Err("Label is too long (max 100 characters)".into());
    }

    let scope = app.asset_protocol_scope();
    let sessions = paths
        .into_iter()
        .map(|path| {
            let canonical = fs::canonicalize(Path::new(&path))
                .map_err(|error| format!("Cannot resolve imported file: {error}"))?;
            crate::safety::check_file_safe(&canonical)?;
            if scope.is_forbidden(&canonical) {
                return Err(
                    "This path was removed from the Library during this app session and cannot be re-authorized. Copy the audio to a new path or restart DepoAudio before importing it again."
                        .into(),
                );
            }
            let extension = playable_library_extension(&canonical)?;
            let size = fs::metadata(&canonical)
                .map_err(|e| format!("Cannot inspect imported file: {e}"))?
                .len();
            let source_name = canonical
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("imported")
                .to_string();
            let path = canonical.to_string_lossy().to_string();
            Ok(Session {
                id: Uuid::new_v4().to_string(),
                date: Utc::now().format("%Y-%m-%d").to_string(),
                source_file: path.clone(),
                source_name,
                participants: vec![Participant {
                    label: label_trimmed.clone(),
                    files: vec![LibFile {
                        path,
                        format: extension,
                        size,
                    }],
                }],
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let imported_paths: Vec<String> = sessions.iter().flat_map(session_paths).collect();

    mutate_library(app, state, move |lib| {
        let idx = crate::persistence::find_case_idx(&lib.cases, &sanitized_name);
        let case = if let Some(index) = idx {
            // Importing into an existing case re-activates it if archived —
            // the user is explicitly adding content, so it should be visible.
            lib.cases[index].archived = false;
            &mut lib.cases[index]
        } else {
            lib.cases.push(Case {
                id: Uuid::new_v4().to_string(),
                name: sanitized_name,
                created_at: Utc::now().to_rfc3339(),
                archived: false,
                sessions: vec![],
            });
            lib.cases
                .last_mut()
                .ok_or_else(|| "Failed to create library case".to_string())?
        };
        case.sessions.extend(sessions);
        Ok(())
    })?;

    let mut failures = 0usize;
    let mut first_error = None;
    for path in imported_paths {
        if let Err(error) = scope.allow_file(&path) {
            failures += 1;
            first_error.get_or_insert_with(|| error.to_string());
        }
    }
    Ok(LibraryMutationResult {
        changed: true,
        warning: (failures > 0).then(|| {
            format!(
                "The audio was imported, but {failures} file(s) could not be authorized for playback. Restart DepoAudio to rebuild access from the Library. First error: {}",
                first_error.unwrap_or_else(|| "unknown asset-scope error".into())
            )
        }),
    })
}

// ── Prefs commands ────────────────────────────────────────────────────────────

#[tauri::command]
pub fn prefs_get(_app: AppHandle, state: State<'_, AppState>) -> Result<Prefs, String> {
    prefs_ready(&state)?;
    let prefs = state.prefs.lock().unwrap_or_else(|e| e.into_inner());
    Ok(prefs.clone())
}

#[tauri::command]
pub fn prefs_set(app: AppHandle, state: State<'_, AppState>, patch: serde_json::Value) -> Result<bool, String> {
    prefs_ready(&state)?;
    let mut current = state.prefs.lock().unwrap_or_else(|e| e.into_inner());
    let candidate = crate::persistence::merge_prefs(&current, patch)?;
    let json = serde_json::to_string_pretty(&candidate).map_err(|e| format!("Failed to serialize preferences: {e}"))?;
    let path = prefs_path(&app)?;
    crate::persistence::atomic_write(&path, json.as_bytes())?;
    *current = candidate;
    Ok(true)
}

// ── Shell / opener commands ───────────────────────────────────────────────────

#[tauri::command]
pub fn show_in_folder(app: AppHandle, path: String) -> Result<(), String> {
    app.opener().reveal_item_in_dir(&path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, path: &str) -> Session {
        Session {
            id: id.into(),
            date: String::new(),
            source_file: path.into(),
            source_name: path.into(),
            participants: vec![Participant {
                label: "Speaker".into(),
                files: vec![LibFile {
                    path: path.into(),
                    format: "wav".into(),
                    size: 1,
                }],
            }],
        }
    }

    #[test]
    fn library_import_extension_allowlist_is_audio_only() {
        for path in ["recording.wav", "recording.MP3", "recording.aiff", "recording.opus"] {
            assert!(playable_library_extension(Path::new(path)).is_ok(), "rejected {path}");
        }
        for path in ["secrets.txt", "library.json", "no-extension", "recording.ftr"] {
            assert!(playable_library_extension(Path::new(path)).is_err(), "accepted {path}");
        }
    }

    #[test]
    fn asset_revocation_keeps_paths_still_referenced_elsewhere() {
        let shared = "C:\\audio\\shared.wav";
        let unique = "C:\\audio\\unique.wav";
        let library = Library {
            version: 1,
            cases: vec![Case {
                id: "remaining".into(),
                name: "Remaining".into(),
                created_at: String::new(),
                archived: false,
                sessions: vec![session("s2", shared)],
            }],
        };

        assert_eq!(
            unreferenced_paths(&library, vec![shared.into(), unique.into(), unique.into()]),
            vec![unique.to_string()]
        );
    }
}
