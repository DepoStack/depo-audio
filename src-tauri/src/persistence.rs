use std::fs;
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

pub(crate) fn save_library(app: &AppHandle, lib: &Library) {
    let path = lib_path(app);
    if let Some(parent) = path.parent() { let _ = fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string_pretty(lib) {
        let _ = fs::write(path, json);
    }
}

pub(crate) fn save_to_library(app: &AppHandle, state: &tauri::State<'_, AppState>, job: &ConvertJob, files: &[OutputFile]) {
    let mut lib = state.library.lock().unwrap();

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

    let case_name_lower = case_name.to_lowercase();
    let case_idx = lib.cases.iter().position(|c| c.name.to_lowercase() == case_name_lower);
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

    save_library(app, &lib);
}
