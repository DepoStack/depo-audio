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

/// The case a conversion files under: the user's explicit name, or one
/// inferred from the source filename when blank.
pub(crate) fn resolve_case_name(job: &ConvertJob, source_name: &str) -> String {
    job.case_name.clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| infer_case_name(source_name))
}

/// How output files map to library participants per output mode: split gets
/// one participant per channel (user label or "Channel N"), keep is labeled
/// "Original", everything else "Stereo Mix".
pub(crate) fn build_participants(job: &ConvertJob, files: &[OutputFile]) -> Vec<Participant> {
    if job.mode == "split" {
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
    }
}

/// Apply a JSON patch to prefs: top-level keys replace wholesale (camelCase,
/// matching the wire format). If the patched result no longer deserializes as
/// valid Prefs, the current prefs are kept unchanged — a bad patch can never
/// corrupt settings.
pub(crate) fn merge_prefs(current: &Prefs, patch: serde_json::Value) -> Prefs {
    if let Ok(mut cur_val) = serde_json::to_value(current.clone()) {
        if let serde_json::Value::Object(ref mut map) = cur_val {
            if let serde_json::Value::Object(patch_map) = patch {
                for (k, v) in patch_map { map.insert(k, v); }
            }
        }
        if let Ok(updated) = serde_json::from_value::<Prefs>(cur_val) {
            return updated;
        }
    }
    current.clone()
}

pub(crate) fn save_to_library(app: &AppHandle, state: &tauri::State<'_, AppState>, job: &ConvertJob, files: &[OutputFile]) {
    let mut lib = state.library.lock().unwrap_or_else(|e| e.into_inner());

    let source_name = basename(&job.src_path);
    let case_name = resolve_case_name(job, &source_name);
    let participants = build_participants(job, files);

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

    // ── Conversion → library filing ─────────────────────────────────────────

    fn job(mode: &str, case_name: Option<&str>, labels: &[&str]) -> ConvertJob {
        serde_json::from_value(serde_json::json!({
            "id": "t", "srcPath": "/deps/Smith_2024-01-15.wav", "outDir": "",
            "mode": mode, "format": "mp3", "rate": "48000",
            "labels": labels, "chanVols": [], "normalize": false, "trim": false,
            "fade": false, "fadeDur": 0.5, "hpf": false, "caseName": case_name,
        })).expect("valid job JSON")
    }

    fn out(path: &str, size: u64) -> OutputFile {
        OutputFile { name: basename(path), path: path.into(), size }
    }

    #[test]
    fn case_name_prefers_users_explicit_name() {
        assert_eq!(resolve_case_name(&job("stereo", Some("Doe v Roe"), &[]), "Smith_2024-01-15.wav"), "Doe v Roe");
    }

    #[test]
    fn blank_case_name_falls_back_to_inference() {
        // Empty and whitespace-only names both defer to filename inference
        assert_eq!(resolve_case_name(&job("stereo", None, &[]), "Smith_2024-01-15.wav"), "Smith");
        assert_eq!(resolve_case_name(&job("stereo", Some("   "), &[]), "Smith_2024-01-15.wav"), "Smith");
    }

    #[test]
    fn split_mode_files_one_participant_per_channel() {
        let j = job("split", None, &["Judge", "Witness"]);
        let files = vec![out("/o/a_Judge.mp3", 10), out("/o/a_Witness.mp3", 20), out("/o/a_ch3.mp3", 30)];
        let p = build_participants(&j, &files);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].label, "Judge");
        assert_eq!(p[1].label, "Witness");
        // Channels beyond the provided labels get a positional fallback
        assert_eq!(p[2].label, "Channel 3");
        assert_eq!(p[0].files[0].format, "mp3");
        assert_eq!(p[2].files[0].size, 30);
    }

    #[test]
    fn keep_mode_labels_original_and_stereo_labels_mix() {
        let files = vec![out("/o/a.mp3", 10)];
        assert_eq!(build_participants(&job("keep", None, &[]), &files)[0].label, "Original");
        assert_eq!(build_participants(&job("stereo", None, &[]), &files)[0].label, "Stereo Mix");
    }

    // ── Prefs patch-merge ───────────────────────────────────────────────────

    #[test]
    fn merge_prefs_replaces_top_level_keys() {
        let cur = Prefs::default();
        let next = merge_prefs(&cur, serde_json::json!({ "theme": "dark", "mp3Bitrate": 320 }));
        assert_eq!(next.theme, "dark");
        assert_eq!(next.mp3_bitrate, 320);
        // Untouched fields survive
        assert_eq!(next.rate, "48000");
        assert_eq!(next.labels, cur.labels);
    }

    #[test]
    fn merge_prefs_uses_camel_case_wire_names() {
        // snake_case keys are NOT the wire format — they are ignored as unknown
        let next = merge_prefs(&Prefs::default(), serde_json::json!({ "mp3_bitrate": 320 }));
        assert_eq!(next.mp3_bitrate, 192);
    }

    #[test]
    fn merge_prefs_rejects_type_mismatches_wholesale() {
        // A patch that breaks deserialization must not corrupt ANY field —
        // the entire patch is dropped, even its valid keys
        let next = merge_prefs(&Prefs::default(), serde_json::json!({ "theme": "dark", "fadeDur": "not-a-number" }));
        assert_eq!(next.theme, "system");
        assert_eq!(next.fade_dur, 0.5);
    }

    #[test]
    fn merge_prefs_non_object_patch_is_a_noop() {
        let next = merge_prefs(&Prefs::default(), serde_json::json!([1, 2, 3]));
        assert_eq!(next.theme, "system");
    }
}
