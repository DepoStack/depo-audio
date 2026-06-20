use std::fs;
use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::helpers::{basename, infer_case_name};
use crate::types::*;

// ── Path helpers ──────────────────────────────────────────────────────────────

pub(crate) fn lib_path(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from(".")).join("library.json")
}

pub(crate) fn prefs_path(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from(".")).join("prefs.json")
}

// ── Library persistence ───────────────────────────────────────────────────────

pub(crate) fn load_library(app: &AppHandle) -> Library {
    let path = lib_path(app);
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Write bytes to `path` atomically: a uniquely-named temp file is written and
/// fsync'd, then renamed over the destination. Readers never observe a
/// truncated or partial file, and a crash mid-write leaves the previous file
/// intact. Falls back to a direct write so data is never lost outright.
pub(crate) fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let wrote = fs::OpenOptions::new()
        .write(true).create(true).truncate(true)
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|_| fs::rename(&tmp, path));
    if let Err(primary) = wrote {
        let _ = fs::remove_file(&tmp);
        fs::write(path, bytes)
            .map_err(|_| format!("Failed to write {}: {}", path.display(), primary))?;
    }
    Ok(())
}

pub(crate) fn save_library(app: &AppHandle, lib: &Library) -> Result<(), String> {
    let path = lib_path(app);
    let json = serde_json::to_string_pretty(lib)
        .map_err(|e| format!("Failed to serialize library: {}", e))?;
    atomic_write(&path, json.as_bytes())
}

/// Find a case by name (case-insensitive), regardless of archived state. Both
/// auto-filing (below) and manual import use this so they agree on whether a
/// case already exists — matching on different rules previously let an import
/// silently create a duplicate of an archived case.
pub(crate) fn find_case_idx(cases: &[Case], name: &str) -> Option<usize> {
    let lower = name.to_lowercase();
    cases.iter().position(|c| c.name.to_lowercase() == lower)
}

pub(crate) fn save_to_library(app: &AppHandle, state: &tauri::State<'_, AppState>, job: &ConvertJob, files: &[OutputFile]) {
    let mut lib = state.library.lock().unwrap_or_else(|e| e.into_inner());

    let source_name = basename(&job.src_path);
    let case_name = job.case_name.clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| infer_case_name(&source_name));

    let participants = if job.mode == "split" {
        files.iter().enumerate().map(|(i, f)| Participant {
            label: job.labels.get(i).cloned().unwrap_or_else(|| format!("Channel {}", i + 1)),
            files: vec![LibFile { path: f.path.clone(), format: job.format.clone(), size: f.size }],
        }).collect()
    } else {
        let label = if job.mode == "keep" { "Original".to_string() } else { "Stereo Mix".to_string() };
        files.iter().map(|f| Participant {
            label: label.clone(),
            files: vec![LibFile { path: f.path.clone(), format: job.format.clone(), size: f.size }],
        }).collect()
    };

    let session = Session {
        id: Uuid::new_v4().to_string(),
        date: Utc::now().format("%Y-%m-%d").to_string(),
        source_file: job.src_path.clone(),
        source_name,
        participants,
    };

    let case_idx = find_case_idx(&lib.cases, &case_name);
    if let Some(idx) = case_idx {
        lib.cases[idx].sessions.push(session);
    } else {
        lib.cases.push(Case {
            id: Uuid::new_v4().to_string(),
            name: case_name,
            created_at: Utc::now().to_rfc3339(),
            archived: false,
            sessions: vec![session],
        });
    }

    // Conversion already succeeded and the files exist on disk; a failed
    // library save should not fail the conversion itself.
    let _ = save_library(app, &lib);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn case(name: &str, archived: bool) -> Case {
        Case { id: name.into(), name: name.into(), created_at: String::new(), archived, sessions: vec![] }
    }

    #[test]
    fn find_case_idx_is_case_insensitive_and_archived_agnostic() {
        let cases = vec![case("Smith", true), case("Jones", false)];
        // Matches an archived case (so import and auto-file agree → no dupes)
        assert_eq!(find_case_idx(&cases, "smith"), Some(0));
        assert_eq!(find_case_idx(&cases, "JONES"), Some(1));
        assert_eq!(find_case_idx(&cases, "Doe"), None);
    }
}
