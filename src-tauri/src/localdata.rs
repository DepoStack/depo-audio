//! Local data controls (DA-07): where DepoAudio keeps its own data, a backup
//! of the library and transcripts as a zip, restoring that backup, and
//! clearing everything DepoAudio stored.
//!
//! DepoAudio's own files live in the app data folder: `library.json` (case
//! names, sessions and the paths of converted files), `prefs.json`
//! (settings), unfinished-save files left by an interrupted atomic write, and
//! legacy model files from older builds. Transcripts, bookmarks, playback
//! speed and the theme live in the app's web view storage; the frontend owns
//! that part (`src/lib/localData.js`).
//!
//! Clearing removes only those exact names. It never follows a path recorded
//! in the library, so source recordings, converted outputs, export records and
//! backup zips are never touched.
//!
//! The backup is a plain zip with stored (uncompressed) entries, written and
//! read here without a new dependency. Import accepts only an unmodified
//! DepoAudio backup: known entry names, stored entries, checked CRC-32 and
//! size limits.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::types::{Case, Library};

pub(crate) const BACKUP_FORMAT: &str = "depoaudio-backup";
pub(crate) const BACKUP_FORMAT_VERSION: u64 = 1;
/// Largest backup DepoAudio writes or reads. The library itself is capped at
/// 16 MiB by persistence.rs; transcripts and bookmarks share the rest.
pub(crate) const MAX_BACKUP_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LIBRARY_ENTRY_BYTES: usize = 16 * 1024 * 1024;

const ENTRY_INFO: &str = "depoaudio-backup.json";
const ENTRY_LIBRARY: &str = "library.json";
const ENTRY_TRANSCRIPTS: &str = "transcripts.json";
const ENTRY_BOOKMARKS: &str = "bookmarks.json";
const ENTRY_README: &str = "README.txt";
const KNOWN_ENTRIES: [&str; 5] = [
    ENTRY_INFO,
    ENTRY_LIBRARY,
    ENTRY_TRANSCRIPTS,
    ENTRY_BOOKMARKS,
    ENTRY_README,
];

const NOT_A_BACKUP: &str = "This file is not a DepoAudio backup, or it was changed after export. Choose a .zip that DepoAudio exported, without unpacking or re-zipping it.";

const README_TEXT: &str = "DepoAudio backup\r\n\r\n\
This zip holds your DepoAudio case library (case names, sessions and the paths of converted files),\r\n\
the transcripts you edited in the Player, and your bookmarks. It does not hold any audio.\r\n\
The contents are not encrypted. Keep this file where you keep the recordings.\r\n\r\n\
To restore it, open DepoAudio > Settings > Your data > Import backup.\r\n\
Import adds what is missing and never removes anything already in DepoAudio.\r\n";

// ── What DepoAudio stores in app data ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StoredItem {
    /// Path relative to the app data folder, with `/` separators.
    pub name: String,
    /// library | prefs | unfinished-save | legacy-model
    pub kind: &'static str,
    pub bytes: u64,
}

fn is_unfinished_save(name: &str) -> bool {
    // persistence::atomic_write names its temp file
    // `<stem>.tmp.<pid>.<32 hex uuid>` next to library.json or prefs.json.
    let Some(rest) = name
        .strip_prefix("library.tmp.")
        .or_else(|| name.strip_prefix("prefs.tmp."))
    else {
        return false;
    };
    let Some((pid, id)) = rest.split_once('.') else {
        return false;
    };
    !pid.is_empty()
        && pid.chars().all(|c| c.is_ascii_digit())
        && id.len() == 32
        && id.chars().all(|c| c.is_ascii_hexdigit())
}

fn regular_file_size(path: &Path) -> Option<u64> {
    let metadata = fs::symlink_metadata(path).ok()?;
    metadata.file_type().is_file().then_some(metadata.len())
}

/// Every DepoAudio-owned regular file in `dir`, by exact name. Nothing else in
/// the folder is listed, and symlinks or folders with these names are skipped.
pub(crate) fn stored_items(dir: &Path) -> Vec<StoredItem> {
    let mut items = Vec::new();
    for (name, kind) in [("library.json", "library"), ("prefs.json", "prefs")] {
        if let Some(bytes) = regular_file_size(&dir.join(name)) {
            items.push(StoredItem {
                name: name.into(),
                kind,
                bytes,
            });
        }
    }
    if let Ok(entries) = fs::read_dir(dir) {
        let mut unfinished: Vec<StoredItem> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                if !is_unfinished_save(&name) {
                    return None;
                }
                let bytes = regular_file_size(&entry.path())?;
                Some(StoredItem {
                    name,
                    kind: "unfinished-save",
                    bytes,
                })
            })
            .collect();
        unfinished.sort_by(|a, b| a.name.cmp(&b.name));
        items.extend(unfinished);
    }
    let models = dir.join("models");
    for filename in crate::models::legacy_model_filenames() {
        if let Some(bytes) = regular_file_size(&models.join(filename)) {
            items.push(StoredItem {
                name: format!("models/{filename}"),
                kind: "legacy-model",
                bytes,
            });
        }
    }
    items
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ClearOutcome {
    pub removed: Vec<String>,
    /// (name, reason) for each file that could not be removed.
    pub failed: Vec<(String, String)>,
}

/// Remove every item `stored_items` lists, and nothing else.
pub(crate) fn clear_app_data_in(dir: &Path) -> ClearOutcome {
    let mut outcome = ClearOutcome::default();
    for item in stored_items(dir) {
        let path: PathBuf = item
            .name
            .split('/')
            .fold(dir.to_path_buf(), |path, part| path.join(part));
        match fs::remove_file(&path) {
            Ok(()) => outcome.removed.push(item.name),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => outcome.failed.push((item.name, error.kind().to_string())),
        }
    }
    outcome
}

// ── Minimal stored zip ───────────────────────────────────────────────────────

fn crc32(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut table = [0u32; 256];
        for (index, slot) in table.iter_mut().enumerate() {
            let mut value = index as u32;
            for _ in 0..8 {
                value = if value & 1 == 1 {
                    0xEDB8_8320 ^ (value >> 1)
                } else {
                    value >> 1
                };
            }
            *slot = value;
        }
        table
    });
    !bytes.iter().fold(!0u32, |crc, byte| {
        table[((crc ^ u32::from(*byte)) & 0xFF) as usize] ^ (crc >> 8)
    })
}

fn dos_date_time(time: &DateTime<Utc>) -> (u16, u16) {
    let year = time.year().clamp(1980, 2107) as u16;
    let date = ((year - 1980) << 9) | ((time.month() as u16) << 5) | time.day() as u16;
    let clock = ((time.hour() as u16) << 11) | ((time.minute() as u16) << 5) | (time.second() as u16 / 2);
    (clock, date)
}

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL: u32 = 0x0605_4b50;
const UTF8_NAMES: u16 = 0x0800;

/// A zip of stored entries (method 0) with UTF-8 names. Any zip tool can
/// open it; DepoAudio reads it back with `read_stored_zip`.
pub(crate) fn write_stored_zip(entries: &[(&str, &[u8])], time: &DateTime<Utc>) -> Result<Vec<u8>, String> {
    let (clock, date) = dos_date_time(time);
    let mut out = Vec::new();
    let mut central = Vec::new();
    let to_u32 = |value: usize| u32::try_from(value).map_err(|_| "The backup is too large".to_string());
    for (name, data) in entries {
        let offset = to_u32(out.len())?;
        let crc = crc32(data);
        let size = to_u32(data.len())?;
        let name_len = u16::try_from(name.len()).map_err(|_| "Backup entry name is too long".to_string())?;

        out.extend_from_slice(&LOCAL_HEADER.to_le_bytes());
        for value in [20u16, UTF8_NAMES, 0, clock, date] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, size, size] {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);

        central.extend_from_slice(&CENTRAL_HEADER.to_le_bytes());
        for value in [20u16, 20, UTF8_NAMES, 0, clock, date] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        for value in [crc, size, size] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        // name length, extra length, comment length, disk, internal attributes
        for value in [name_len, 0, 0, 0, 0] {
            central.extend_from_slice(&value.to_le_bytes());
        }
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let central_offset = to_u32(out.len())?;
    let central_size = to_u32(central.len())?;
    let count = u16::try_from(entries.len()).map_err(|_| "Too many backup entries".to_string())?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&END_OF_CENTRAL.to_le_bytes());
    for value in [0u16, 0, count, count] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&central_size.to_le_bytes());
    out.extend_from_slice(&central_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

fn le_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn le_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Read a zip of stored entries whose names are all in `allowed`. Compressed,
/// encrypted, split, zip64, duplicate or unknown entries are refused, and
/// every entry's CRC-32 is checked.
pub(crate) fn read_stored_zip(bytes: &[u8], allowed: &[&str]) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let invalid = || NOT_A_BACKUP.to_string();
    if bytes.len() < 22 || bytes.len() as u64 > MAX_BACKUP_BYTES {
        return Err(invalid());
    }
    let search_from = bytes.len().saturating_sub(22 + 0xFFFF);
    let eocd = (search_from..=bytes.len() - 22)
        .rev()
        .find(|&at| le_u32(bytes, at) == Some(END_OF_CENTRAL))
        .ok_or_else(invalid)?;
    let disk = le_u16(bytes, eocd + 4).ok_or_else(invalid)?;
    let central_disk = le_u16(bytes, eocd + 6).ok_or_else(invalid)?;
    let disk_entries = le_u16(bytes, eocd + 8).ok_or_else(invalid)?;
    let count = le_u16(bytes, eocd + 10).ok_or_else(invalid)?;
    let central_size = le_u32(bytes, eocd + 12).ok_or_else(invalid)? as usize;
    let central_offset = le_u32(bytes, eocd + 16).ok_or_else(invalid)? as usize;
    if disk != 0 || central_disk != 0 || disk_entries != count || count as usize > allowed.len() {
        return Err(invalid());
    }
    if central_offset.checked_add(central_size).is_none_or(|end| end > eocd) {
        return Err(invalid());
    }

    let mut entries = BTreeMap::new();
    let mut at = central_offset;
    for _ in 0..count {
        if le_u32(bytes, at) != Some(CENTRAL_HEADER) {
            return Err(invalid());
        }
        let flags = le_u16(bytes, at + 8).ok_or_else(invalid)?;
        let method = le_u16(bytes, at + 10).ok_or_else(invalid)?;
        let crc = le_u32(bytes, at + 16).ok_or_else(invalid)?;
        let compressed = le_u32(bytes, at + 20).ok_or_else(invalid)? as usize;
        let size = le_u32(bytes, at + 24).ok_or_else(invalid)? as usize;
        let name_len = le_u16(bytes, at + 28).ok_or_else(invalid)? as usize;
        let extra_len = le_u16(bytes, at + 30).ok_or_else(invalid)? as usize;
        let comment_len = le_u16(bytes, at + 32).ok_or_else(invalid)? as usize;
        let local = le_u32(bytes, at + 42).ok_or_else(invalid)? as usize;
        // Bit 0: encrypted. Bit 3: sizes in a trailing data descriptor.
        if flags & 0x0009 != 0 || method != 0 || compressed != size {
            return Err(invalid());
        }
        let name = bytes.get(at + 46..at + 46 + name_len).ok_or_else(invalid)?;
        let name = std::str::from_utf8(name).map_err(|_| invalid())?.to_string();
        if !allowed.contains(&name.as_str()) || entries.contains_key(&name) {
            return Err(invalid());
        }

        if le_u32(bytes, local) != Some(LOCAL_HEADER) {
            return Err(invalid());
        }
        let local_name_len = le_u16(bytes, local + 26).ok_or_else(invalid)? as usize;
        let local_extra_len = le_u16(bytes, local + 28).ok_or_else(invalid)? as usize;
        if bytes.get(local + 30..local + 30 + local_name_len) != Some(name.as_bytes()) {
            return Err(invalid());
        }
        let start = local + 30 + local_name_len + local_extra_len;
        let data = start
            .checked_add(size)
            .filter(|&end| end <= central_offset)
            .and_then(|end| bytes.get(start..end))
            .ok_or_else(invalid)?;
        if crc32(data) != crc {
            return Err(invalid());
        }
        entries.insert(name, data.to_vec());
        at += 46 + name_len + extra_len + comment_len;
    }
    if at != central_offset + central_size {
        return Err(invalid());
    }
    Ok(entries)
}

// ── Backup contents ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackupCounts {
    pub cases: usize,
    pub sessions: usize,
    pub transcripts: usize,
    pub bookmarks: usize,
}

fn counts(
    library: &Library,
    transcripts: &serde_json::Map<String, serde_json::Value>,
    bookmarks: &[serde_json::Value],
) -> BackupCounts {
    BackupCounts {
        cases: library.cases.len(),
        sessions: library.cases.iter().map(|case| case.sessions.len()).sum(),
        transcripts: transcripts.len(),
        bookmarks: bookmarks.len(),
    }
}

/// Build the backup zip. `transcripts` maps a track path to its segment array
/// and `bookmarks` is the Player's bookmark array, both as the frontend keeps
/// them; the frontend validates their shape again when it restores them.
pub(crate) fn build_backup(
    library: &Library,
    transcripts: &serde_json::Value,
    bookmarks: &serde_json::Value,
    app_version: &str,
    time: &DateTime<Utc>,
) -> Result<(Vec<u8>, BackupCounts), String> {
    let transcript_map = transcripts
        .as_object()
        .ok_or_else(|| "Transcripts must be an object keyed by track".to_string())?;
    let bookmark_list = bookmarks
        .as_array()
        .ok_or_else(|| "Bookmarks must be a list".to_string())?;
    let summary = counts(library, transcript_map, bookmark_list);
    let info = serde_json::json!({
        "format": BACKUP_FORMAT,
        "formatVersion": BACKUP_FORMAT_VERSION,
        "appVersion": app_version,
        "exportedAt": time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "contents": summary,
    });
    let pretty = |value: &serde_json::Value| {
        serde_json::to_vec_pretty(value).map_err(|error| format!("Cannot prepare the backup: {error}"))
    };
    let info = pretty(&info)?;
    let library_json =
        serde_json::to_vec_pretty(library).map_err(|error| format!("Cannot prepare the backup: {error}"))?;
    if library_json.len() > MAX_LIBRARY_ENTRY_BYTES {
        return Err("The library is larger than a backup can hold".into());
    }
    let transcripts_json = pretty(transcripts)?;
    let bookmarks_json = pretty(bookmarks)?;
    let zip = write_stored_zip(
        &[
            (ENTRY_INFO, &info),
            (ENTRY_README, README_TEXT.as_bytes()),
            (ENTRY_LIBRARY, &library_json),
            (ENTRY_TRANSCRIPTS, &transcripts_json),
            (ENTRY_BOOKMARKS, &bookmarks_json),
        ],
        time,
    )?;
    if zip.len() as u64 > MAX_BACKUP_BYTES {
        return Err("The library and transcripts are larger than a backup can hold (64 MB)".into());
    }
    Ok((zip, summary))
}

#[derive(Debug)]
pub(crate) struct Backup {
    pub library: Library,
    pub transcripts: serde_json::Value,
    pub bookmarks: serde_json::Value,
}

pub(crate) fn parse_backup(bytes: &[u8]) -> Result<Backup, String> {
    let entries = read_stored_zip(bytes, &KNOWN_ENTRIES)?;
    let json = |name: &str| -> Result<Option<serde_json::Value>, String> {
        entries
            .get(name)
            .map(|data| serde_json::from_slice(data).map_err(|_| NOT_A_BACKUP.to_string()))
            .transpose()
    };
    let info = json(ENTRY_INFO)?.ok_or_else(|| NOT_A_BACKUP.to_string())?;
    if info.get("format").and_then(|v| v.as_str()) != Some(BACKUP_FORMAT) {
        return Err(NOT_A_BACKUP.into());
    }
    match info.get("formatVersion").and_then(|v| v.as_u64()) {
        Some(BACKUP_FORMAT_VERSION) => {}
        Some(_) => {
            return Err("This backup was made by a newer DepoAudio. Update DepoAudio, then import it again.".into())
        }
        None => return Err(NOT_A_BACKUP.into()),
    }
    let library_bytes = entries.get(ENTRY_LIBRARY).ok_or_else(|| NOT_A_BACKUP.to_string())?;
    if library_bytes.len() > MAX_LIBRARY_ENTRY_BYTES {
        return Err(NOT_A_BACKUP.into());
    }
    let library: Library = serde_json::from_slice(library_bytes)
        .map_err(|_| "The library in this backup could not be read.".to_string())?;
    let transcripts = json(ENTRY_TRANSCRIPTS)?.unwrap_or_else(|| serde_json::json!({}));
    let bookmarks = json(ENTRY_BOOKMARKS)?.unwrap_or_else(|| serde_json::json!([]));
    if !transcripts.is_object() || !bookmarks.is_array() {
        return Err(NOT_A_BACKUP.into());
    }
    Ok(Backup {
        library,
        transcripts,
        bookmarks,
    })
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct MergeReport {
    pub cases_added: usize,
    pub sessions_added: usize,
    /// Sessions already in the library (same session id), left as they are.
    pub sessions_skipped: usize,
}

/// Add the backup's cases and sessions to `current`. Nothing in `current` is
/// removed or changed, except that a case gains the sessions it lacks. Cases
/// match by name (case-insensitive, as auto-filing does); sessions match by id.
/// Every case name and file entry is validated before anything is added.
pub(crate) fn merge_library(current: &mut Library, incoming: Library) -> Result<(MergeReport, Vec<String>), String> {
    for case in &incoming.cases {
        crate::helpers::sanitize_case_name(&case.name)
            .map_err(|error| format!("The backup has a case name DepoAudio cannot use: {error}"))?;
        for file in case
            .sessions
            .iter()
            .flat_map(|session| &session.participants)
            .flat_map(|participant| &participant.files)
        {
            crate::commands::playable_library_extension(Path::new(&file.path))
                .map_err(|_| "The backup lists a Library file that is not a supported audio file.".to_string())?;
        }
    }

    let mut report = MergeReport::default();
    let mut added_paths = Vec::new();
    let mut session_ids: HashSet<String> = current
        .cases
        .iter()
        .flat_map(|case| &case.sessions)
        .map(|session| session.id.clone())
        .collect();
    let case_ids: HashSet<String> = current.cases.iter().map(|case| case.id.clone()).collect();

    for case in incoming.cases {
        let name = crate::helpers::sanitize_case_name(&case.name)?;
        let mut sessions = Vec::new();
        for mut session in case.sessions {
            if session.id.trim().is_empty() {
                session.id = Uuid::new_v4().to_string();
            }
            if !session_ids.insert(session.id.clone()) {
                report.sessions_skipped += 1;
                continue;
            }
            added_paths.extend(
                session
                    .participants
                    .iter()
                    .flat_map(|participant| &participant.files)
                    .map(|file| file.path.clone()),
            );
            sessions.push(session);
        }
        report.sessions_added += sessions.len();
        match crate::persistence::find_case_idx(&current.cases, &name) {
            Some(index) => current.cases[index].sessions.extend(sessions),
            None => {
                let id = if case.id.trim().is_empty() || case_ids.contains(&case.id) {
                    Uuid::new_v4().to_string()
                } else {
                    case.id
                };
                current.cases.push(Case {
                    id,
                    name,
                    created_at: case.created_at,
                    archived: case.archived,
                    sessions,
                });
                report.cases_added += 1;
            }
        }
    }
    if current.version == 0 {
        current.version = incoming.version.max(1);
    }
    Ok((report, added_paths))
}

/// Write a new file only: an existing file is never replaced.
pub(crate) fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                "A file with that name already exists. DepoAudio never replaces a file; choose a new name.".to_string()
            } else {
                format!("Cannot create the backup file: {}", error.kind())
            }
        })?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(format!("Cannot write the backup file: {}", error.kind()));
    }
    Ok(())
}

pub(crate) fn is_zip_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LibFile, Participant, Session};

    fn test_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("depoaudio_localdata_{label}_{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn time() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-13T10:20:30Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn session(id: &str, path: &str) -> Session {
        Session {
            id: id.into(),
            date: "2026-11-13".into(),
            source_file: path.into(),
            source_name: "source.wav".into(),
            participants: vec![Participant {
                label: "Witness".into(),
                files: vec![LibFile {
                    path: path.into(),
                    format: "wav".into(),
                    size: 4,
                }],
            }],
        }
    }

    fn library(cases: Vec<(&str, &str, Vec<Session>)>) -> Library {
        Library {
            version: 1,
            cases: cases
                .into_iter()
                .map(|(id, name, sessions)| Case {
                    id: id.into(),
                    name: name.into(),
                    created_at: "2026-11-13T00:00:00Z".into(),
                    archived: false,
                    sessions,
                })
                .collect(),
        }
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn clear_removes_only_depoaudio_files_and_never_user_audio() {
        let dir = test_dir("clear");
        let outputs = test_dir("outputs");
        for name in [
            "library.json",
            "prefs.json",
            "library.tmp.123.0123456789abcdef0123456789abcdef",
            "prefs.tmp.9.fedcba9876543210fedcba9876543210",
        ] {
            fs::write(dir.join(name), b"{}").unwrap();
        }
        // Not DepoAudio's: a recording, an output and look-alike names.
        for name in [
            "recording.wav",
            "library.json.bak",
            "library.tmp.notes",
            "prefs.tmp.1.short",
            "Smith v Jones.zip",
        ] {
            fs::write(dir.join(name), b"keep").unwrap();
        }
        fs::create_dir_all(dir.join("models")).unwrap();
        fs::write(dir.join("models/flashsr.onnx"), b"legacy").unwrap();
        fs::write(dir.join("models/someone-elses.onnx"), b"keep").unwrap();
        let output = outputs.join("Smith_Witness.wav");
        fs::write(&output, b"RIFF").unwrap();
        fs::write(outputs.join("Smith_Witness.wav.depoaudio.json"), b"{}").unwrap();

        let listed: Vec<String> = stored_items(&dir).into_iter().map(|item| item.name).collect();
        assert_eq!(
            listed,
            vec![
                "library.json",
                "prefs.json",
                "library.tmp.123.0123456789abcdef0123456789abcdef",
                "prefs.tmp.9.fedcba9876543210fedcba9876543210",
                "models/flashsr.onnx",
            ]
        );

        let outcome = clear_app_data_in(&dir);
        assert_eq!(outcome.removed, listed);
        assert!(outcome.failed.is_empty());
        for kept in [
            "recording.wav",
            "library.json.bak",
            "library.tmp.notes",
            "prefs.tmp.1.short",
            "Smith v Jones.zip",
            "models/someone-elses.onnx",
        ] {
            assert!(dir.join(kept).exists(), "{kept} was removed");
        }
        assert!(output.exists());
        assert!(outputs.join("Smith_Witness.wav.depoaudio.json").exists());
        assert!(stored_items(&dir).is_empty());
        // A second clear has nothing to do.
        assert_eq!(clear_app_data_in(&dir), ClearOutcome::default());

        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(outputs);
    }

    #[cfg(unix)]
    #[test]
    fn clear_never_follows_a_symlink_named_like_library_json() {
        let dir = test_dir("symlink");
        let elsewhere = test_dir("symlink_target");
        let target = elsewhere.join("recording.wav");
        fs::write(&target, b"RIFF").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("library.json")).unwrap();

        assert!(stored_items(&dir).is_empty());
        assert!(clear_app_data_in(&dir).removed.is_empty());
        assert!(target.exists());

        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(elsewhere);
    }

    #[test]
    fn backup_round_trips_library_transcripts_and_bookmarks() {
        let lib = library(vec![("c1", "Smith v Jones", vec![session("s1", "/out/a.wav")])]);
        let transcripts = serde_json::json!({ "/out/a.wav": [{ "id": "x", "start": 1.5, "speaker": "Q", "text": "Synthetic line" }] });
        let bookmarks = serde_json::json!([{ "time": 2.0, "trackPath": "/out/a.wav", "label": "0:02" }]);

        let (zip, summary) = build_backup(&lib, &transcripts, &bookmarks, "1.0.3", &time()).unwrap();
        assert_eq!(
            summary,
            BackupCounts {
                cases: 1,
                sessions: 1,
                transcripts: 1,
                bookmarks: 1
            }
        );
        let backup = parse_backup(&zip).unwrap();
        assert_eq!(backup.library.cases[0].name, "Smith v Jones");
        assert_eq!(
            backup.library.cases[0].sessions[0].participants[0].files[0].path,
            "/out/a.wav"
        );
        assert_eq!(backup.transcripts, transcripts);
        assert_eq!(backup.bookmarks, bookmarks);

        let entries = read_stored_zip(&zip, &KNOWN_ENTRIES).unwrap();
        let info: serde_json::Value = serde_json::from_slice(&entries[ENTRY_INFO]).unwrap();
        assert_eq!(info["format"], BACKUP_FORMAT);
        assert_eq!(info["formatVersion"], 1);
        assert_eq!(info["appVersion"], "1.0.3");
        assert_eq!(info["exportedAt"], "2026-11-13T10:20:30Z");
        assert!(String::from_utf8_lossy(&entries[ENTRY_README]).contains("not encrypted"));
    }

    #[test]
    fn backup_is_a_zip_other_tools_can_open() {
        let Ok(probe) = std::process::Command::new("unzip").arg("-v").output() else {
            eprintln!("unzip not installed; skipping the external zip check");
            return;
        };
        if !probe.status.success() {
            return;
        }
        let dir = test_dir("unzip");
        let lib = library(vec![("c1", "Doe v Roe", vec![session("s1", "/out/b.mp3")])]);
        let (zip, _) = build_backup(&lib, &serde_json::json!({}), &serde_json::json!([]), "1.0.3", &time()).unwrap();
        let path = dir.join("backup.zip");
        fs::write(&path, &zip).unwrap();
        let tested = std::process::Command::new("unzip")
            .arg("-t")
            .arg(&path)
            .output()
            .unwrap();
        assert!(tested.status.success(), "{}", String::from_utf8_lossy(&tested.stdout));
        let listing = std::process::Command::new("unzip")
            .arg("-Z1")
            .arg(&path)
            .output()
            .unwrap();
        let names = String::from_utf8_lossy(&listing.stdout);
        for name in KNOWN_ENTRIES {
            assert!(names.lines().any(|line| line == name), "{name} missing from {names}");
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn reader_refuses_changed_or_foreign_zips() {
        let lib = library(vec![("c1", "Case", vec![session("s1", "/out/a.wav")])]);
        let (zip, _) = build_backup(&lib, &serde_json::json!({}), &serde_json::json!([]), "1.0.3", &time()).unwrap();

        // A flipped byte inside an entry fails its CRC-32.
        let mut corrupted = zip.clone();
        let at = corrupted.windows(5).position(|w| w == b"Case\"").unwrap();
        corrupted[at] = b'K';
        assert!(parse_backup(&corrupted).unwrap_err().contains("not a DepoAudio backup"));

        // Truncated, empty and unrelated input.
        assert!(parse_backup(&zip[..zip.len() - 10]).is_err());
        assert!(parse_backup(b"").is_err());
        assert!(parse_backup(b"PK\x03\x04 definitely not a backup").is_err());

        // A well-formed zip with an unknown entry name.
        let foreign = write_stored_zip(&[("payload.exe", b"MZ")], &time()).unwrap();
        assert!(parse_backup(&foreign).is_err());

        // A zip of known names without the format marker.
        let unmarked = write_stored_zip(&[(ENTRY_LIBRARY, br#"{"version":1,"cases":[]}"#)], &time()).unwrap();
        assert!(parse_backup(&unmarked).is_err());

        // A compressed entry (method 8) is refused.
        let mut compressed = write_stored_zip(&[(ENTRY_README, b"x")], &time()).unwrap();
        compressed[8] = 8;
        let central = compressed
            .windows(4)
            .position(|w| w == CENTRAL_HEADER.to_le_bytes())
            .unwrap();
        compressed[central + 10] = 8;
        assert!(read_stored_zip(&compressed, &KNOWN_ENTRIES).is_err());

        // Duplicate names are refused.
        let duplicate = write_stored_zip(&[(ENTRY_README, b"a"), (ENTRY_README, b"b")], &time()).unwrap();
        assert!(read_stored_zip(&duplicate, &KNOWN_ENTRIES).is_err());
    }

    #[test]
    fn a_newer_backup_format_asks_for_an_update() {
        let info = br#"{"format":"depoaudio-backup","formatVersion":2}"#;
        let zip = write_stored_zip(
            &[(ENTRY_INFO, info), (ENTRY_LIBRARY, br#"{"version":1,"cases":[]}"#)],
            &time(),
        )
        .unwrap();
        assert!(parse_backup(&zip).unwrap_err().contains("newer DepoAudio"));
    }

    #[test]
    fn merge_adds_missing_cases_and_sessions_and_removes_nothing() {
        let mut current = library(vec![
            ("c1", "Smith v Jones", vec![session("s1", "/out/a.wav")]),
            ("c2", "Kept Case", vec![session("s2", "/out/k.wav")]),
        ]);
        let incoming = library(vec![
            // Same case by name (different case), one known and one new session.
            (
                "other-id",
                "smith V JONES",
                vec![session("s1", "/out/a.wav"), session("s3", "/out/c.wav")],
            ),
            // A new case whose id collides with an existing case id.
            ("c2", "Doe v Roe", vec![session("s4", "/out/d.mp3")]),
        ]);

        let (report, added) = merge_library(&mut current, incoming).unwrap();
        assert_eq!(
            report,
            MergeReport {
                cases_added: 1,
                sessions_added: 2,
                sessions_skipped: 1
            }
        );
        assert_eq!(added, vec!["/out/c.wav".to_string(), "/out/d.mp3".to_string()]);
        assert_eq!(current.cases.len(), 3);
        assert_eq!(current.cases[0].name, "Smith v Jones");
        assert_eq!(
            current.cases[0]
                .sessions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>(),
            ["s1", "s3"]
        );
        assert_eq!(current.cases[1].sessions.len(), 1);
        assert_eq!(current.cases[2].name, "Doe v Roe");
        assert_ne!(current.cases[2].id, "c2");

        // Importing the same backup twice adds nothing more.
        let again = library(vec![("x", "Doe v Roe", vec![session("s4", "/out/d.mp3")])]);
        let (report, added) = merge_library(&mut current, again).unwrap();
        assert_eq!(report.sessions_skipped, 1);
        assert_eq!(report.sessions_added + report.cases_added, 0);
        assert!(added.is_empty());
    }

    #[test]
    fn merge_rejects_invalid_entries_before_changing_anything() {
        let mut current = library(vec![("c1", "Case", vec![])]);
        let before = serde_json::to_string(&current).unwrap();

        let bad_name = library(vec![("n", "///", vec![])]);
        assert!(merge_library(&mut current, bad_name).is_err());

        let not_audio = library(vec![
            ("ok", "Fine", vec![session("s9", "/out/fine.wav")]),
            ("n", "Other", vec![session("s8", "/home/user/.ssh/id_ed25519")]),
        ]);
        assert!(merge_library(&mut current, not_audio).is_err());
        assert_eq!(serde_json::to_string(&current).unwrap(), before);
    }

    #[test]
    fn export_writes_a_new_file_and_never_replaces_one() {
        let dir = test_dir("export");
        let path = dir.join("backup.zip");
        write_new_file(&path, b"first").unwrap();
        let error = write_new_file(&path, b"second").unwrap_err();
        assert!(error.contains("never replaces"));
        assert_eq!(fs::read(&path).unwrap(), b"first");
        assert!(is_zip_path(Path::new("C:\\Backups\\DepoAudio.ZIP")));
        assert!(!is_zip_path(Path::new("/tmp/backup.json")));
        let _ = fs::remove_dir_all(dir);
    }
}
