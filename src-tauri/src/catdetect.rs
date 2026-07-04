use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ── Court reporting software detection ──────────────────────────────────────
//
// Scans common installation paths for court reporting CAT software and finds
// audio files (jobs) for easy import into the library. Supports:
//
//   - Stenograph Case CATalyst (.sgmca files)
//   - FTR Gold / For The Record (.trm, .ftr files)
//   - Eclipse CAT (.aes files — flagged as encrypted)
//   - DigitalCAT (.dm files)
//   - CourtSmart (.bwf files)

/// A detected court reporting software installation.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CatSoftware {
    pub name: String,
    pub vendor: String,
    pub path: String,
    pub job_count: usize,
}

/// An audio job found in a CAT software directory.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CatJob {
    pub software: String,
    pub name: String,
    pub path: String,
    pub files: Vec<CatJobFile>,
    pub date_modified: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CatJobFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub format: String,
}

/// Known CAT software with their typical installation/data paths.
struct CatProfile {
    name: &'static str,
    vendor: &'static str,
    extensions: &'static [&'static str],
    /// Paths to search (platform-specific).
    search_paths: Vec<PathBuf>,
}

/// Detect installed court reporting software and their audio files.
pub(crate) fn detect_cat_software(max_depth: usize) -> Vec<CatSoftware> {
    let profiles = build_profiles();
    let mut found = Vec::new();

    for profile in &profiles {
        for search_path in &profile.search_paths {
            if search_path.exists() && search_path.is_dir() {
                let job_count = count_audio_files(search_path, profile.extensions, 0, max_depth);
                if job_count > 0 {
                    found.push(CatSoftware {
                        name: profile.name.into(),
                        vendor: profile.vendor.into(),
                        path: search_path.to_string_lossy().to_string(),
                        job_count,
                    });
                }
            }
        }
    }

    found
}

/// Scan a CAT software directory for importable audio jobs.
/// Path is canonicalized and restricted to known safe locations.
pub(crate) fn scan_cat_jobs(base_path: &str) -> Vec<CatJob> {
    let base = match Path::new(base_path).canonicalize() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    if !base.is_dir() {
        return Vec::new();
    }

    // Security: only allow scanning paths under the user's home/documents or
    // the concrete CAT install directories — never the whole C:\ drive. Roots
    // are canonicalized so the comparison matches `base` (also canonicalized),
    // including Windows \\?\ verbatim prefixes.
    let allowed_roots: Vec<PathBuf> = {
        let home = dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let docs = dirs_next::document_dir().unwrap_or_else(|| home.join("Documents"));
        let mut roots = vec![home, docs];
        for profile in build_profiles() {
            roots.extend(profile.search_paths);
        }
        roots.iter().filter_map(|r| r.canonicalize().ok()).collect()
    };

    let is_allowed = allowed_roots.iter().any(|root| base.starts_with(root));
    if !is_allowed {
        return Vec::new();
    }

    let audio_exts = ["sgmca", "trm", "ftr", "bwf", "dm", "aes", "wav", "mp3", "flac", "m4a"];
    let mut jobs = Vec::new();

    // Walk top-level directories as "jobs" (cases/sessions)
    if let Ok(entries) = fs::read_dir(base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
                let files = find_audio_in_dir(&path, &audio_exts);
                if !files.is_empty() {
                    let date = entry.metadata()
                        .and_then(|m| m.modified())
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Utc> = t.into();
                            dt.format("%Y-%m-%d").to_string()
                        })
                        .unwrap_or_default();

                    // Infer software from file extensions
                    let software = infer_software(&files);

                    jobs.push(CatJob {
                        software,
                        name: dir_name,
                        path: path.to_string_lossy().to_string(),
                        files,
                        date_modified: date,
                    });
                }
            } else if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if audio_exts.contains(&ext.to_lowercase().as_str()) {
                    let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("").to_string();
                    let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                    let date = entry.metadata()
                        .and_then(|m| m.modified())
                        .map(|t| {
                            let dt: chrono::DateTime<chrono::Utc> = t.into();
                            dt.format("%Y-%m-%d").to_string()
                        })
                        .unwrap_or_default();

                    let fmt = ext.to_lowercase();
                    let software = match fmt.as_str() {
                        "sgmca" => "Case CATalyst",
                        "trm" | "ftr" => "FTR Gold",
                        "bwf" => "CourtSmart",
                        "dm" => "DigitalCAT",
                        "aes" => "Eclipse",
                        _ => "Standard",
                    };

                    jobs.push(CatJob {
                        software: software.into(),
                        name,
                        path: path.to_string_lossy().to_string(),
                        files: vec![CatJobFile {
                            path: path.to_string_lossy().to_string(),
                            name: path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
                            size,
                            format: fmt,
                        }],
                        date_modified: date,
                    });
                }
            }
        }
    }

    // Sort by date, newest first
    jobs.sort_by(|a, b| b.date_modified.cmp(&a.date_modified));
    jobs
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn build_profiles() -> Vec<CatProfile> {
    let home = dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let docs = dirs_next::document_dir().unwrap_or_else(|| home.join("Documents"));

    vec![
        CatProfile {
            name: "Case CATalyst",
            vendor: "Stenograph",
            extensions: &["sgmca"],
            search_paths: vec![
                docs.join("CaseCatalyst"),
                docs.join("Case CATalyst"),
                home.join("CaseCatalyst"),
                PathBuf::from("C:\\CaseCatalyst"),
                PathBuf::from("C:\\Program Files\\Stenograph\\CaseCatalyst"),
            ],
        },
        CatProfile {
            name: "FTR Gold",
            vendor: "For The Record",
            extensions: &["trm", "ftr"],
            search_paths: vec![
                docs.join("FTR"),
                docs.join("FTR Gold"),
                PathBuf::from("C:\\FTR"),
                PathBuf::from("C:\\Program Files\\FTR"),
                PathBuf::from("C:\\Program Files (x86)\\FTR"),
            ],
        },
        CatProfile {
            name: "Eclipse",
            vendor: "Advantage Software",
            extensions: &["aes"],
            search_paths: vec![
                docs.join("Eclipse"),
                PathBuf::from("C:\\Eclipse"),
                PathBuf::from("C:\\Program Files\\Eclipse"),
            ],
        },
        CatProfile {
            name: "DigitalCAT",
            vendor: "Stenovations",
            extensions: &["dm"],
            search_paths: vec![
                docs.join("DigitalCAT"),
                PathBuf::from("C:\\DigitalCAT"),
            ],
        },
        CatProfile {
            name: "CourtSmart",
            vendor: "CourtSmart",
            extensions: &["bwf"],
            search_paths: vec![
                docs.join("CourtSmart"),
                PathBuf::from("C:\\CourtSmart"),
            ],
        },
    ]
}

fn count_audio_files(dir: &Path, extensions: &[&str], depth: usize, max_depth: usize) -> usize {
    if depth >= max_depth {
        return 0;
    }
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            // Don't follow symlinks: a symlink cycle could otherwise recurse
            // (within the depth budget) and re-walk large trees redundantly.
            let ft = match entry.file_type() { Ok(ft) => ft, Err(_) => continue };
            if ft.is_symlink() { continue; }
            let path = entry.path();
            if ft.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if extensions.contains(&ext.to_lowercase().as_str()) {
                        count += 1;
                    }
                }
            } else if ft.is_dir() {
                count += count_audio_files(&path, extensions, depth + 1, max_depth);
            }
        }
    }
    count
}

fn find_audio_in_dir(dir: &Path, extensions: &[&str]) -> Vec<CatJobFile> {
    let mut files = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if extensions.contains(&ext.to_lowercase().as_str()) {
                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        files.push(CatJobFile {
                            path: path.to_string_lossy().to_string(),
                            name: path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_string(),
                            size,
                            format: ext.to_lowercase(),
                        });
                    }
                }
            }
        }
    }
    files
}

fn infer_software(files: &[CatJobFile]) -> String {
    for f in files {
        match f.format.as_str() {
            "sgmca" => return "Case CATalyst".into(),
            "trm" | "ftr" => return "FTR Gold".into(),
            "bwf" => return "CourtSmart".into(),
            "dm" => return "DigitalCAT".into(),
            "aes" => return "Eclipse".into(),
            _ => {}
        }
    }
    "Standard".into()
}
