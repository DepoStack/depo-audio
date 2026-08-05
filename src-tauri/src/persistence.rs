use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::Utc;
use tauri::{AppHandle, Manager};
use uuid::Uuid;

use crate::helpers::{basename, infer_case_name};
use crate::types::*;

// ── Path helpers ──────────────────────────────────────────────────────────────

pub(crate) fn lib_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("library.json"))
        .map_err(|e| format!("Cannot resolve app data directory: {e}"))
}

pub(crate) fn prefs_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|dir| dir.join("prefs.json"))
        .map_err(|e| format!("Cannot resolve app data directory: {e}"))
}

// ── Library persistence ───────────────────────────────────────────────────────

const MAX_LIBRARY_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PREFS_BYTES: u64 = 1024 * 1024;

fn read_json_or_default<T>(path: &Path, label: &str, max_bytes: u64) -> Result<T, String>
where
    T: serde::de::DeserializeOwned + Default,
{
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(format!("Cannot read {label} file: {e}")),
    };
    let size = file
        .metadata()
        .map_err(|e| format!("Cannot inspect {label} file: {e}"))?
        .len();
    if size > max_bytes {
        return Err(format!("{label} file exceeds the {max_bytes}-byte safety limit"));
    }

    // The metadata check avoids a large initial allocation; `take` also keeps
    // the read bounded if another process grows the file after that check.
    let mut json = String::with_capacity(size as usize);
    file.take(max_bytes + 1)
        .read_to_string(&mut json)
        .map_err(|e| format!("Cannot read {label} file: {e}"))?;
    if json.len() as u64 > max_bytes {
        return Err(format!("{label} file exceeds the {max_bytes}-byte safety limit"));
    }
    serde_json::from_str(&json).map_err(|e| format!("{label} file is corrupt or incompatible: {e}"))
}

pub(crate) fn load_library(app: &AppHandle) -> Result<Library, String> {
    read_json_or_default(&lib_path(app)?, "Library", MAX_LIBRARY_BYTES)
}

pub(crate) fn load_prefs(app: &AppHandle) -> Result<Prefs, String> {
    read_json_or_default(&prefs_path(app)?, "Preferences", MAX_PREFS_BYTES)
}

/// Write bytes without ever truncating the live file. The uniquely named temp
/// file is fsync'd before a single replace-existing rename. Rust implements
/// that rename with `MoveFileExW(MOVEFILE_REPLACE_EXISTING)` on Windows and
/// atomic `rename(2)` semantics on Unix, so a failed promotion leaves the live
/// file untouched and a successful one never exposes a missing-file gap.
pub(crate) fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Cannot create persistence directory: {e}"))?;
    }
    let tmp = path.with_extension(format!("tmp.{}.{}", std::process::id(), Uuid::new_v4().simple()));
    let wrote = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        });
    if let Err(error) = wrote {
        let _ = fs::remove_file(&tmp);
        return Err(format!("Failed to write {}: {}", path.display(), error));
    }

    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            let _ = fs::remove_file(&tmp);
            return Err(format!(
                "Refusing to replace non-file persistence target {}",
                path.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            return Err(format!("Cannot inspect {}: {}", path.display(), error));
        }
    }

    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(format!("Failed to atomically replace {}: {error}", path.display()));
    }

    // On Unix, fsync the containing directory so the rename itself is durable
    // across a sudden power loss. Windows does not permit the same
    // directory-open pattern through std::fs.
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Err(error) = fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            // The atomic rename has already committed the candidate. Returning
            // an error now would leave callers with stale in-memory state even
            // though the new bytes are live on disk. Preserve consistency and
            // report only the reduced crash-durability guarantee.
            eprintln!("[persistence] Could not sync persistence directory after commit: {error}");
        }
    }
    Ok(())
}

pub(crate) fn save_library(app: &AppHandle, lib: &Library) -> Result<(), String> {
    let path = lib_path(app)?;
    let json = serde_json::to_string_pretty(lib).map_err(|e| format!("Failed to serialize library: {}", e))?;
    atomic_write(&path, json.as_bytes())
}

fn storage_ready(error: &std::sync::Mutex<Option<String>>, label: &str) -> Result<(), String> {
    let error = error.lock().unwrap_or_else(|e| e.into_inner());
    match error.as_ref() {
        Some(message) => Err(format!(
            "{label} storage is read-only because it could not be loaded: {message}"
        )),
        None => Ok(()),
    }
}

pub(crate) fn library_ready(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    storage_ready(&state.library_load_error, "Library")
}

pub(crate) fn prefs_ready(state: &tauri::State<'_, AppState>) -> Result<(), String> {
    storage_ready(&state.prefs_load_error, "Preferences")
}

/// Persist a candidate value before making it visible in memory. This keeps a
/// failed disk write from being committed later by an unrelated successful
/// mutation.
fn commit_candidate<T>(
    current: &mut T,
    candidate: T,
    persist: impl FnOnce(&T) -> Result<(), String>,
) -> Result<(), String> {
    persist(&candidate)?;
    *current = candidate;
    Ok(())
}

pub(crate) fn mutate_library<R>(
    app: &AppHandle,
    state: &tauri::State<'_, AppState>,
    mutate: impl FnOnce(&mut Library) -> Result<R, String>,
) -> Result<R, String> {
    library_ready(state)?;
    let mut current = state.library.lock().unwrap_or_else(|e| e.into_inner());
    let mut candidate = current.clone();
    let result = mutate(&mut candidate)?;
    commit_candidate(&mut *current, candidate, |next| save_library(app, next))?;
    Ok(result)
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
    job.case_name
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| infer_case_name(source_name))
}

/// How output files map to library participants per output mode: split gets
/// one participant per channel (user label or "Channel N"), keep is labeled
/// "Original", everything else "Stereo Mix".
pub(crate) fn build_participants(job: &ConvertJob, files: &[OutputFile]) -> Vec<Participant> {
    if job.mode == "split" {
        files
            .iter()
            .enumerate()
            .map(|(i, f)| Participant {
                label: job
                    .labels
                    .get(i)
                    .cloned()
                    .unwrap_or_else(|| format!("Channel {}", i + 1)),
                files: vec![LibFile {
                    path: f.path.clone(),
                    format: job.format.clone(),
                    size: f.size,
                }],
            })
            .collect()
    } else {
        let label = if job.mode == "keep" {
            "Original".to_string()
        } else {
            "Stereo Mix".to_string()
        };
        files
            .iter()
            .map(|f| Participant {
                label: label.clone(),
                files: vec![LibFile {
                    path: f.path.clone(),
                    format: job.format.clone(),
                    size: f.size,
                }],
            })
            .collect()
    }
}

/// Apply a JSON patch to prefs: top-level keys replace wholesale (camelCase,
/// matching the wire format). Invalid types, unknown keys, and non-object
/// patches are rejected explicitly so callers cannot report a successful save
/// when the requested setting was actually ignored.
pub(crate) fn merge_prefs(current: &Prefs, patch: serde_json::Value) -> Result<Prefs, String> {
    let mut current_value =
        serde_json::to_value(current).map_err(|error| format!("Cannot serialize current preferences: {error}"))?;
    let current_map = current_value
        .as_object_mut()
        .ok_or_else(|| "Current preferences have an invalid wire shape".to_string())?;
    let patch_map = patch
        .as_object()
        .ok_or_else(|| "Preference update must be a JSON object".to_string())?;

    for (key, value) in patch_map {
        if !current_map.contains_key(key) {
            return Err(format!("Unknown preference key: {key}"));
        }
        current_map.insert(key.clone(), value.clone());
    }

    serde_json::from_value(current_value).map_err(|error| format!("Invalid preference update: {error}"))
}

pub(crate) fn save_to_library(
    app: &AppHandle,
    state: &tauri::State<'_, AppState>,
    job: &ConvertJob,
    files: &[OutputFile],
) -> Result<(), String> {
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

    mutate_library(app, state, move |lib| {
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
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("depoaudio_persistence_{label}_{}", Uuid::new_v4().simple()))
    }

    #[test]
    fn missing_json_is_a_clean_first_run_but_corrupt_json_is_an_error() {
        let dir = test_dir("load");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.json");

        let missing: Library = read_json_or_default(&path, "Library", MAX_LIBRARY_BYTES).unwrap();
        assert!(missing.cases.is_empty());

        fs::write(&path, b"{ definitely not valid json").unwrap();
        let corrupt = read_json_or_default::<Library>(&path, "Library", MAX_LIBRARY_BYTES);
        assert!(corrupt.is_err());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn oversized_json_fails_closed_before_deserialization() {
        let dir = test_dir("oversized");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.json");
        fs::write(&path, vec![b' '; 33]).unwrap();

        let error = read_json_or_default::<Library>(&path, "Library", 32).unwrap_err();
        assert!(error.contains("exceeds the 32-byte safety limit"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn atomic_write_replaces_complete_contents() {
        let dir = test_dir("atomic");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        fs::write(&path, b"old contents that are longer").unwrap();

        atomic_write(&path, b"new").unwrap();
        atomic_write(&path, b"final state").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"final state");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_persist_does_not_mutate_in_memory_state() {
        let mut current = vec!["original"];
        let candidate = vec!["changed"];
        let result = commit_candidate(&mut current, candidate, |_| Err("disk full".into()));
        assert!(result.is_err());
        assert_eq!(current, vec!["original"]);
    }

    fn case(name: &str, archived: bool) -> Case {
        Case {
            id: name.into(),
            name: name.into(),
            created_at: String::new(),
            archived,
            sessions: vec![],
        }
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
        }))
        .expect("valid job JSON")
    }

    fn out(path: &str, size: u64) -> OutputFile {
        OutputFile {
            name: basename(path),
            path: path.into(),
            size,
        }
    }

    #[test]
    fn case_name_prefers_users_explicit_name() {
        assert_eq!(
            resolve_case_name(&job("stereo", Some("Doe v Roe"), &[]), "Smith_2024-01-15.wav"),
            "Doe v Roe"
        );
    }

    #[test]
    fn blank_case_name_falls_back_to_inference() {
        // Empty and whitespace-only names both defer to filename inference
        assert_eq!(
            resolve_case_name(&job("stereo", None, &[]), "Smith_2024-01-15.wav"),
            "Smith"
        );
        assert_eq!(
            resolve_case_name(&job("stereo", Some("   "), &[]), "Smith_2024-01-15.wav"),
            "Smith"
        );
    }

    #[test]
    fn split_mode_files_one_participant_per_channel() {
        let j = job("split", None, &["Judge", "Witness"]);
        let files = vec![
            out("/o/a_Judge.mp3", 10),
            out("/o/a_Witness.mp3", 20),
            out("/o/a_ch3.mp3", 30),
        ];
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
        assert_eq!(
            build_participants(&job("stereo", None, &[]), &files)[0].label,
            "Stereo Mix"
        );
    }

    // ── Prefs patch-merge ───────────────────────────────────────────────────

    #[test]
    fn merge_prefs_replaces_top_level_keys() {
        let cur = Prefs::default();
        let next = merge_prefs(&cur, serde_json::json!({ "theme": "dark", "mp3Bitrate": 320 })).unwrap();
        assert_eq!(next.theme, "dark");
        assert_eq!(next.mp3_bitrate, 320);
        // Untouched fields survive
        assert_eq!(next.rate, "48000");
        assert_eq!(next.labels, cur.labels);
    }

    #[test]
    fn merge_prefs_uses_camel_case_wire_names() {
        // snake_case keys are NOT the wire format and must fail explicitly.
        let error = merge_prefs(&Prefs::default(), serde_json::json!({ "mp3_bitrate": 320 })).unwrap_err();
        assert!(error.contains("Unknown preference key"));
    }

    #[test]
    fn merge_prefs_rejects_type_mismatches_wholesale() {
        // A patch that breaks deserialization is rejected wholesale.
        let error = merge_prefs(
            &Prefs::default(),
            serde_json::json!({ "theme": "dark", "fadeDur": "not-a-number" }),
        )
        .unwrap_err();
        assert!(error.contains("Invalid preference update"));
    }

    #[test]
    fn merge_prefs_rejects_non_object_patch() {
        let error = merge_prefs(&Prefs::default(), serde_json::json!([1, 2, 3])).unwrap_err();
        assert!(error.contains("must be a JSON object"));
    }
}
