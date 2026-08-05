use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;
use serde::{Deserialize, Serialize};

const AUDIO_EXTENSIONS: &[&str] = &["sgmca", "trm", "ftr", "bwf", "dm", "aes", "wav", "mp3", "flac", "m4a"];
pub(crate) const DEFAULT_SCAN_DEPTH: usize = 5;
const MAX_SCAN_DEPTH: usize = 20;
const FTR_SESSION_GAP_100NS: u64 = 15 * 60 * 10_000_000;
static FTR_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(.+)_(\d{8})-(\d{4,6})_([0-9a-f]{16})\.(trm|ftr)$").expect("the FTR filename regex is valid")
});

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
    /// Supported audio-file count over the same tree depth used by job scan.
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
    let max_depth = normalized_scan_depth(max_depth);

    for profile in &profiles {
        for search_path in &profile.search_paths {
            if search_path.exists() && search_path.is_dir() {
                let (profile_file_count, job_count) =
                    count_profile_audio_files(search_path, profile.extensions, max_depth);
                if profile_file_count > 0 {
                    // The UI can import every supported audio file returned by
                    // `scan_cat_jobs`, not only the profile's identifying
                    // extension. Count that same set over the same traversal.
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
pub(crate) fn scan_cat_jobs(base_path: &str, max_depth: usize) -> Vec<CatJob> {
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
        // A failed home-directory lookup must fail closed. Falling back to
        // `/` maps to a drive root on Windows and would authorize scanning the
        // entire disk.
        let mut roots = Vec::new();
        if let Some(home) = dirs_next::home_dir() {
            roots.push(home);
        }
        if let Some(docs) = dirs_next::document_dir() {
            roots.push(docs);
        }
        for profile in build_profiles() {
            roots.extend(profile.search_paths);
        }
        roots.iter().filter_map(|r| r.canonicalize().ok()).collect()
    };

    let is_allowed = allowed_roots.iter().any(|root| base.starts_with(root));
    if !is_allowed {
        return Vec::new();
    }

    scan_cat_jobs_in(&base, normalized_scan_depth(max_depth))
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
            search_paths: vec![docs.join("DigitalCAT"), PathBuf::from("C:\\DigitalCAT")],
        },
        CatProfile {
            name: "CourtSmart",
            vendor: "CourtSmart",
            extensions: &["bwf"],
            search_paths: vec![docs.join("CourtSmart"), PathBuf::from("C:\\CourtSmart")],
        },
    ]
}

#[derive(Clone)]
struct DiscoveredFile {
    file: CatJobFile,
    modified: Option<SystemTime>,
}

#[derive(Clone)]
struct FtrNameParts {
    location: String,
    date: String,
    time: String,
    filetime: String,
}

fn normalized_scan_depth(max_depth: usize) -> usize {
    if max_depth == 0 {
        DEFAULT_SCAN_DEPTH
    } else {
        max_depth.min(MAX_SCAN_DEPTH)
    }
}

/// Visit supported files with exactly the same depth semantics for detection
/// counts and job enumeration. `max_depth = 1` includes files directly under
/// `dir`; each additional level admits one more directory layer.
fn walk_audio_files(
    dir: &Path,
    extensions: &[&str],
    depth: usize,
    max_depth: usize,
    visit: &mut impl FnMut(&Path, &fs::DirEntry),
) {
    if depth >= max_depth {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        // Never follow symlinks: cycles and links outside an allowed root must
        // not expand the scan despite the recursion depth budget.
        let file_type = match entry.file_type() {
            Ok(file_type) if !file_type.is_symlink() => file_type,
            _ => continue,
        };
        let path = entry.path();
        if file_type.is_file() {
            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };
            if extensions.contains(&ext.to_ascii_lowercase().as_str()) {
                visit(&path, &entry);
            }
        } else if file_type.is_dir() {
            walk_audio_files(&path, extensions, depth + 1, max_depth, visit);
        }
    }
}

#[cfg(test)]
fn count_audio_files(dir: &Path, extensions: &[&str], depth: usize, max_depth: usize) -> usize {
    let mut count = 0usize;
    walk_audio_files(dir, extensions, depth, max_depth, &mut |_, _| {
        count = count.saturating_add(1);
    });
    count
}

fn count_profile_audio_files(dir: &Path, profile_extensions: &[&str], max_depth: usize) -> (usize, usize) {
    let (mut profile_count, mut importable_count) = (0usize, 0usize);
    walk_audio_files(dir, AUDIO_EXTENSIONS, 0, max_depth, &mut |path, _| {
        importable_count = importable_count.saturating_add(1);
        if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| profile_extensions.contains(&extension.to_ascii_lowercase().as_str()))
        {
            profile_count = profile_count.saturating_add(1);
        }
    });
    (profile_count, importable_count)
}

fn scan_cat_jobs_in(base: &Path, max_depth: usize) -> Vec<CatJob> {
    let mut files_by_directory: BTreeMap<PathBuf, Vec<DiscoveredFile>> = BTreeMap::new();
    walk_audio_files(base, AUDIO_EXTENSIONS, 0, max_depth, &mut |path, entry| {
        let Some(parent) = path.parent() else {
            return;
        };
        let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
            return;
        };
        let metadata = entry.metadata().ok();
        files_by_directory
            .entry(parent.to_path_buf())
            .or_default()
            .push(DiscoveredFile {
                file: CatJobFile {
                    path: path.to_string_lossy().to_string(),
                    name: path
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_default(),
                    size: metadata.as_ref().map_or(0, fs::Metadata::len),
                    format: ext.to_ascii_lowercase(),
                },
                modified: metadata.and_then(|metadata| metadata.modified().ok()),
            });
    });

    let mut jobs = Vec::new();
    for (directory, files) in files_by_directory {
        jobs.extend(jobs_for_directory(base, &directory, files));
    }

    jobs.sort_by(|a, b| {
        b.date_modified
            .cmp(&a.date_modified)
            .then_with(|| natural_cmp(&a.name, &b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    jobs
}

fn jobs_for_directory(base: &Path, directory: &Path, files: Vec<DiscoveredFile>) -> Vec<CatJob> {
    let (ftr_files, other_files): (Vec<_>, Vec<_>) = files
        .into_iter()
        .partition(|file| matches!(file.file.format.as_str(), "trm" | "ftr"));
    let directory_name = directory
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Court recording".into());

    let mut jobs = Vec::new();
    if !ftr_files.is_empty() {
        let groups = group_ftr_sessions(ftr_files);
        let qualify_name = groups.len() > 1 || !other_files.is_empty();
        for (session, files) in groups {
            let name = if qualify_name {
                match session {
                    Some(parts) => format!("{} - {} {}-{}", directory_name, parts.location, parts.date, parts.time),
                    None => format!("{directory_name} - FTR recordings"),
                }
            } else {
                directory_name.clone()
            };
            jobs.push(make_job(name, directory.to_path_buf(), files));
        }
    }

    if directory == base {
        // Preserve the existing top-level behavior for ordinary audio: each
        // file is independently importable. FTR/TRM is the exception because
        // one recording session is deliberately split into many chunks.
        for file in other_files {
            let name = Path::new(&file.file.name)
                .file_stem()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| file.file.name.clone());
            let path = PathBuf::from(&file.file.path);
            jobs.push(make_job(name, path, vec![file]));
        }
    } else if !other_files.is_empty() {
        let name = if jobs.is_empty() {
            directory_name
        } else {
            format!("{directory_name} - other audio")
        };
        jobs.push(make_job(name, directory.to_path_buf(), other_files));
    }

    jobs
}

fn group_ftr_sessions(files: Vec<DiscoveredFile>) -> Vec<(Option<FtrNameParts>, Vec<DiscoveredFile>)> {
    let mut named: BTreeMap<String, Vec<DiscoveredFile>> = BTreeMap::new();
    let mut unnamed = Vec::new();

    for file in files {
        if let Some(parts) = parse_ftr_name(&file.file.name) {
            let key = format!("{}\0{}", parts.location.to_ascii_lowercase(), parts.date);
            named.entry(key).or_default().push(file);
        } else {
            unnamed.push(file);
        }
    }

    let mut groups = Vec::new();
    for mut files in named.into_values() {
        files.sort_by(|a, b| compare_cat_files(&a.file, &b.file));
        let mut current_run: Vec<DiscoveredFile> = Vec::new();
        let mut previous_filetime = None;

        for file in files {
            let parts = parse_ftr_name(&file.file.name).expect("named FTR groups contain canonical filenames");
            let filetime =
                u64::from_str_radix(&parts.filetime, 16).expect("the FTR regex accepts hexadecimal FILETIME");
            if previous_filetime.is_some_and(|previous| filetime.saturating_sub(previous) > FTR_SESSION_GAP_100NS)
                && !current_run.is_empty()
            {
                let run_parts = parse_ftr_name(&current_run[0].file.name)
                    .expect("canonical FTR run starts with a canonical filename");
                groups.push((Some(run_parts), std::mem::take(&mut current_run)));
            }
            previous_filetime = Some(filetime);
            current_run.push(file);
        }

        if !current_run.is_empty() {
            let run_parts =
                parse_ftr_name(&current_run[0].file.name).expect("canonical FTR run starts with a canonical filename");
            groups.push((Some(run_parts), current_run));
        }
    }

    // Vendor-renamed chunks commonly sit beside canonical chunks. If there is
    // only one unambiguous chronological run, keep those files in that
    // same batch rather than silently splitting the recording.
    if groups.len() == 1 {
        if let Some((_, files)) = groups.first_mut() {
            files.append(&mut unnamed);
        }
    }

    if !unnamed.is_empty() {
        groups.push((None, unnamed));
    }
    groups
}

fn make_job(name: String, path: PathBuf, mut files: Vec<DiscoveredFile>) -> CatJob {
    files.sort_by(|a, b| compare_cat_files(&a.file, &b.file));
    let date_modified = files
        .iter()
        .filter_map(|file| file.modified)
        .max()
        .map(|modified| {
            let datetime: chrono::DateTime<chrono::Utc> = modified.into();
            datetime.format("%Y-%m-%d").to_string()
        })
        .unwrap_or_default();
    let files: Vec<_> = files.into_iter().map(|file| file.file).collect();
    let software = infer_software(&files);

    CatJob {
        software,
        name,
        path: path.to_string_lossy().to_string(),
        files,
        date_modified,
    }
}

fn parse_ftr_name(name: &str) -> Option<FtrNameParts> {
    let captures = FTR_NAME_RE.captures(name)?;
    Some(FtrNameParts {
        location: captures.get(1)?.as_str().to_string(),
        date: captures.get(2)?.as_str().to_string(),
        time: captures.get(3)?.as_str().to_string(),
        filetime: captures.get(4)?.as_str().to_ascii_lowercase(),
    })
}

fn compare_cat_files(a: &CatJobFile, b: &CatJobFile) -> Ordering {
    match (parse_ftr_name(&a.name), parse_ftr_name(&b.name)) {
        (Some(a_parts), Some(b_parts)) => a_parts
            .location
            .to_ascii_lowercase()
            .cmp(&b_parts.location.to_ascii_lowercase())
            .then_with(|| a_parts.date.cmp(&b_parts.date))
            .then_with(|| a_parts.filetime.cmp(&b_parts.filetime))
            .then_with(|| natural_cmp(&a.name, &b.name)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => natural_cmp(&a.name, &b.name),
    }
    .then_with(|| a.path.cmp(&b.path))
}

fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a_chars: Vec<_> = a.chars().collect();
    let b_chars: Vec<_> = b.chars().collect();
    let (mut a_index, mut b_index) = (0, 0);

    while a_index < a_chars.len() && b_index < b_chars.len() {
        if a_chars[a_index].is_ascii_digit() && b_chars[b_index].is_ascii_digit() {
            let a_start = a_index;
            let b_start = b_index;
            while a_index < a_chars.len() && a_chars[a_index].is_ascii_digit() {
                a_index += 1;
            }
            while b_index < b_chars.len() && b_chars[b_index].is_ascii_digit() {
                b_index += 1;
            }
            let a_digits: String = a_chars[a_start..a_index].iter().collect();
            let b_digits: String = b_chars[b_start..b_index].iter().collect();
            let a_number = a_digits.trim_start_matches('0');
            let b_number = b_digits.trim_start_matches('0');
            let a_number = if a_number.is_empty() { "0" } else { a_number };
            let b_number = if b_number.is_empty() { "0" } else { b_number };
            let order = a_number.len().cmp(&b_number.len()).then_with(|| a_number.cmp(b_number));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a_chars[a_index]
                .to_ascii_lowercase()
                .cmp(&b_chars[b_index].to_ascii_lowercase());
            if order != Ordering::Equal {
                return order;
            }
            a_index += 1;
            b_index += 1;
        }
    }

    a_chars.len().cmp(&b_chars.len()).then_with(|| a.cmp(b))
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

#[cfg(test)]
mod tests {
    use super::*;

    struct TestTree {
        root: PathBuf,
    }

    impl TestTree {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("depoaudio-catdetect-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).expect("create CAT detector test directory");
            Self { root }
        }

        fn write(&self, relative: &str) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create test parent directory");
            }
            fs::write(path, b"audio").expect("write test audio file");
        }
    }

    impl Drop for TestTree {
        fn drop(&mut self) {
            if self
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("depoaudio-catdetect-test-"))
            {
                let _ = fs::remove_dir_all(&self.root);
            }
        }
    }

    fn enumerated_file_count(jobs: &[CatJob]) -> usize {
        jobs.iter().map(|job| job.files.len()).sum()
    }

    #[test]
    fn count_and_enumeration_share_recursive_depth_semantics() {
        let tree = TestTree::new();
        tree.write("root.wav");
        tree.write("case/child.wav");
        tree.write("case/session/grandchild.wav");

        for (depth, expected) in [(1, 1), (2, 2), (3, 3)] {
            let count = count_audio_files(&tree.root, AUDIO_EXTENSIONS, 0, depth);
            let jobs = scan_cat_jobs_in(&tree.root, depth);
            assert_eq!(count, expected, "unexpected detection count at depth {depth}");
            assert_eq!(enumerated_file_count(&jobs), count);
        }
        assert_eq!(count_profile_audio_files(&tree.root, &["trm", "ftr"], 3), (0, 3));
    }

    #[test]
    fn root_and_nested_ftr_chunks_are_batched_and_chronological() {
        let tree = TestTree::new();
        tree.write("CR24_20180621-1459_01d409706d824cb0.trm");
        tree.write("session part 2.trm");
        tree.write("CR24_20180621-1449_01d4096f0757ee50.ftr");
        tree.write("session part 1.trm");
        tree.write("case/session/CR9_20180621-1454_01d4096fbaa19b99.trm");
        tree.write("case/session/CR9_20180621-1449_01d4096f0757ee99.trm");

        let jobs = scan_cat_jobs_in(&tree.root, 3);
        let mut ftr_jobs: Vec<_> = jobs.iter().filter(|job| job.software == "FTR Gold").collect();
        ftr_jobs.sort_by_key(|job| job.files.len());

        assert_eq!(ftr_jobs.len(), 2);
        assert_eq!(
            ftr_jobs[0]
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "CR9_20180621-1449_01d4096f0757ee99.trm",
                "CR9_20180621-1454_01d4096fbaa19b99.trm",
            ]
        );
        assert_eq!(
            ftr_jobs[1]
                .files
                .iter()
                .map(|file| file.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "CR24_20180621-1449_01d4096f0757ee50.ftr",
                "CR24_20180621-1459_01d409706d824cb0.trm",
                "session part 1.trm",
                "session part 2.trm",
            ]
        );
        assert_eq!(enumerated_file_count(&jobs), 6);
        assert_eq!(count_profile_audio_files(&tree.root, &["trm", "ftr"], 3), (6, 6));
    }

    #[test]
    fn distinct_canonical_sessions_in_one_directory_are_separate_batches() {
        let tree = TestTree::new();
        tree.write("CR9_20180621-1449_01d4096f0757ee99.trm");
        tree.write("CR24_20180621-1449_01d4096f0757ee50.trm");

        let jobs = scan_cat_jobs_in(&tree.root, 1);
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|job| job.files.len() == 1));
        assert_eq!(enumerated_file_count(&jobs), 2);
    }

    #[test]
    fn same_room_and_day_split_into_runs_after_a_long_gap() {
        let tree = TestTree::new();
        let first = 0x01d4096f0757ee50u64;
        let second = first + 5 * 60 * 10_000_000;
        let third = second + 16 * 60 * 10_000_000;
        let fourth = third + 5 * 60 * 10_000_000;
        for (time, filetime) in [("0900", first), ("0905", second), ("0921", third), ("0926", fourth)] {
            tree.write(&format!("CR24_20180621-{time}_{filetime:016x}.trm"));
        }
        tree.write("vendor renamed chunk.trm");

        let jobs = scan_cat_jobs_in(&tree.root, 1);
        let mut sizes: Vec<_> = jobs.iter().map(|job| job.files.len()).collect();
        sizes.sort_unstable();

        assert_eq!(sizes, vec![1, 2, 2]);
        assert_eq!(
            jobs.iter()
                .filter(|job| job.files.iter().any(|file| file.name == "vendor renamed chunk.trm"))
                .map(|job| job.files.len())
                .collect::<Vec<_>>(),
            vec![1],
            "renamed chunks must not be guessed into either of two plausible sessions"
        );
        assert_eq!(enumerated_file_count(&jobs), 5);
        assert_eq!(count_profile_audio_files(&tree.root, &["trm", "ftr"], 1), (5, 5));
    }
}
