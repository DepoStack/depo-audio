//! DepoAudio's private work folder for temporary audio copies.
//!
//! Every temporary audio file (the copy of a prefixed SGMCA, the WAV read out
//! of a `.dm`, Merge's decoded and mixed WAVs) is created in
//! `<app cache dir>/work/<run-id>/`, one run folder per app launch:
//!
//! - On Unix the `work` and run folders are mode 0700 and each file is created
//!   only if new, with mode 0600. On Windows the per-user cache folder's
//!   access rules apply; nothing extra is set.
//! - Each run holds `work/<run-id>.lock` locked (std `File::try_lock`) for as
//!   long as the app runs. A run folder whose lock is free belongs to no
//!   running DepoAudio and is removed by the sweep at launch and by "Clear all
//!   DepoAudio data". A locked one is left alone.
//! - Old `depoaudio_*` audio copies that earlier versions left in the system
//!   temporary folder are removed by the same sweep, but only when no other
//!   DepoAudio holds a run lock.
//! - Temporary files are not `sync_all`ed: they are read back by this run only.
//! - On macOS the app cache folder is under `~/Library/Caches`, which Time
//!   Machine's standard exclusions already skip; no extra attribute is set.
//!   (Not checked on a Mac in this change.)

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use uuid::Uuid;

use crate::safety::TempFile;

/// Name of the folder under the app cache folder.
const WORK_FOLDER: &str = "work";

/// Where run folders live. Set once at launch from the app cache folder.
static ROOT: OnceLock<PathBuf> = OnceLock::new();
/// This process's run: its folder and the held lock.
static RUN: Mutex<Option<Run>> = Mutex::new(None);

#[derive(Debug)]
pub(crate) struct Run {
    id: String,
    dir: PathBuf,
    /// Held (locked) for the life of the run; the lock is released when the
    /// process exits, however it exits.
    _lock: File,
}

impl Run {
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }
}

/// What a sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SweepReport {
    /// Run folders removed (their lock was free).
    pub removed_runs: usize,
    /// Run folders left because another DepoAudio holds their lock.
    pub locked_runs: usize,
    /// Old `depoaudio_*` files removed from the system temporary folder.
    pub removed_legacy: usize,
    /// (name, reason) for each item that could not be removed.
    pub failed: Vec<(String, String)>,
}

fn is_run_id(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Create `dir` (and missing parents) and make it private on Unix.
fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        // An existing folder keeps its mode under `create`; set it.
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(dir)?;
    Ok(())
}

/// Open a new file for writing; fails if it exists. Mode 0600 on Unix.
fn create_new_private(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// Start a new run under `root`: create and lock `<id>.lock`, then the
/// `<id>/` folder. The lock exists before the folder, so a sweep never sees
/// this run's folder without its held lock.
pub(crate) fn start_run(root: &Path) -> Result<Run, String> {
    ensure_private_dir(root).map_err(|error| format!("Could not create DepoAudio's private work folder: {error}"))?;
    for _ in 0..4 {
        let id = Uuid::new_v4().simple().to_string();
        let lock_path = root.join(format!("{id}.lock"));
        let lock = create_new_private(&lock_path)
            .map_err(|error| format!("Could not create DepoAudio's private work folder: {error}"))?;
        if lock.try_lock().is_err() {
            // A sweep took it between create and lock; it will remove it.
            continue;
        }
        if !lock_path.exists() {
            // A sweep removed it before the lock was taken: try a new id.
            continue;
        }
        let dir = root.join(&id);
        ensure_private_dir(&dir)
            .map_err(|error| format!("Could not create DepoAudio's private work folder: {error}"))?;
        return Ok(Run { id, dir, _lock: lock });
    }
    Err("Could not create DepoAudio's private work folder: its lock could not be taken".into())
}

/// Is `<root>/<id>.lock` held by a running DepoAudio? `Ok(Some(file))` means
/// it is free and now held by the returned handle; `Ok(None)` means there is
/// no lock file.
fn take_free_lock(lock_path: &Path) -> Result<Option<File>, ()> {
    match OpenOptions::new().read(true).write(true).open(lock_path) {
        Ok(file) => match file.try_lock() {
            Ok(()) => Ok(Some(file)),
            Err(_) => Err(()),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        // Unreadable: treat as held, never remove what cannot be checked.
        Err(_) => Err(()),
    }
}

/// The old temporary audio copy names (system temporary folder, before the
/// work folder): `depoaudio_<32 hex>.sgmca|.wav`, `depoaudio_merged_<32 hex>.wav`
/// and `depoaudio_merge_<32 hex>.wav`.
fn is_legacy_temp_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("depoaudio_") else {
        return false;
    };
    let rest = rest
        .strip_prefix("merged_")
        .or_else(|| rest.strip_prefix("merge_"))
        .unwrap_or(rest);
    let (stem, ext) = match rest.rsplit_once('.') {
        Some(parts) => parts,
        None => return false,
    };
    is_run_id(stem) && (ext == "wav" || (ext == "sgmca" && !name.starts_with("depoaudio_merge")))
}

/// Remove every run folder under `root` whose lock is free, except `own`.
/// When no other run's lock is held, also remove old `depoaudio_*` audio
/// copies directly in `legacy_dir`. Only names DepoAudio makes are touched;
/// symlinks are removed as links, never followed.
pub(crate) fn sweep(root: &Path, own: Option<&str>, legacy_dir: Option<&Path>) -> SweepReport {
    let mut report = SweepReport::default();
    let mut ids = std::collections::BTreeSet::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".lock").unwrap_or(&name);
            if is_run_id(id) && Some(id) != own {
                ids.insert(id.to_string());
            }
        }
    }
    for id in ids {
        let lock_path = root.join(format!("{id}.lock"));
        let lock = match take_free_lock(&lock_path) {
            Ok(lock) => lock,
            Err(()) => {
                report.locked_runs += 1;
                continue;
            }
        };
        let dir = root.join(&id);
        let removed = match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(&dir),
            Ok(_) => fs::remove_file(&dir),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        };
        match removed {
            Ok(()) => report.removed_runs += 1,
            Err(error) => {
                report.failed.push((format!("work/{id}"), error.kind().to_string()));
                continue;
            }
        }
        if let Some(lock) = lock {
            // Release the handle before removing the file (Windows).
            drop(lock);
            if let Err(error) = fs::remove_file(&lock_path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    report
                        .failed
                        .push((format!("work/{id}.lock"), error.kind().to_string()));
                }
            }
        }
    }

    if report.locked_runs == 0 {
        if let Some(legacy_dir) = legacy_dir {
            if let Ok(entries) = fs::read_dir(legacy_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !is_legacy_temp_name(&name) {
                        continue;
                    }
                    let path = entry.path();
                    // A regular file or a link; never a folder.
                    if fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_dir()) {
                        continue;
                    }
                    match fs::remove_file(&path) {
                        Ok(()) => report.removed_legacy += 1,
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => report.failed.push((name, error.kind().to_string())),
                    }
                }
            }
        }
    }
    report
}

fn default_root() -> PathBuf {
    #[cfg(test)]
    {
        // Shared by test processes; leftovers of finished ones are swept
        // the first time a test process uses it.
        let root = std::env::temp_dir().join("depoaudio_test_work");
        sweep(&root, None, None);
        root
    }
    #[cfg(not(test))]
    {
        // Only used if launch did not set the folder; the same place Tauri's
        // app cache folder resolves to.
        dirs_next::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("com.andrewmayes.depoaudio")
            .join(WORK_FOLDER)
    }
}

fn root() -> &'static Path {
    ROOT.get_or_init(default_root)
}

/// At launch: use `<cache_dir>/work` and start this run, then sweep it (and
/// the system temporary folder's old copies) on a background thread so a
/// large leftover copy does not hold up the first window.
pub(crate) fn launch(cache_dir: &Path) {
    let _ = ROOT.set(cache_dir.join(WORK_FOLDER));
    let mut run = RUN.lock().unwrap_or_else(|error| error.into_inner());
    if run.is_none() {
        match start_run(root()) {
            Ok(started) => *run = Some(started),
            Err(error) => eprintln!("[workdir] {error}"),
        }
    }
    drop(run);
    std::thread::spawn(|| {
        let report = sweep_now();
        if !report.failed.is_empty() {
            eprintln!(
                "[workdir] {} leftover temporary item(s) could not be removed",
                report.failed.len()
            );
        }
    });
}

/// For "Clear all DepoAudio data": the same sweep, keeping this run.
pub(crate) fn sweep_now() -> SweepReport {
    let own = RUN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(|run| run.id.clone());
    sweep(root(), own.as_deref(), Some(&std::env::temp_dir()))
}

#[cfg(test)]
struct ThreadRun(Run);

#[cfg(test)]
impl Drop for ThreadRun {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0.dir);
        let _ = fs::remove_file(self.0.dir.with_extension("lock"));
    }
}

#[cfg(test)]
thread_local! {
    /// Tests run in parallel threads; each gets its own run folder so a test
    /// can check that its folder is empty afterwards.
    static THREAD_RUN: std::cell::RefCell<Option<ThreadRun>> = const { std::cell::RefCell::new(None) };
}

/// This run's folder, starting the run if launch could not.
#[cfg(test)]
fn run_dir() -> Result<PathBuf, String> {
    THREAD_RUN.with(|cell| {
        let mut run = cell.borrow_mut();
        if run.is_none() {
            *run = Some(ThreadRun(start_run(root())?));
        }
        let dir = run.as_ref().map(|run| run.0.dir().to_path_buf()).unwrap_or_default();
        ensure_private_dir(&dir).map_err(|error| error.to_string())?;
        Ok(dir)
    })
}

/// This run's folder, starting the run if launch could not.
#[cfg(not(test))]
fn run_dir() -> Result<PathBuf, String> {
    let mut run = RUN.lock().unwrap_or_else(|error| error.into_inner());
    if run.is_none() {
        *run = Some(start_run(root())?);
    }
    let dir = run.as_ref().map(|run| run.dir().to_path_buf()).unwrap_or_default();
    // Recreate it if something outside the app removed it.
    ensure_private_dir(&dir).map_err(|error| format!("Could not create DepoAudio's private work folder: {error}"))?;
    Ok(dir)
}

/// A new, empty temporary file in this run's folder, named
/// `<prefix><uuid>.<ext>`. The guard removes it when dropped.
pub(crate) fn new_temp_file(prefix: &str, ext: &str) -> Result<(TempFile, File), String> {
    let dir = run_dir()?;
    let path = dir.join(format!("{prefix}{}.{ext}", Uuid::new_v4().simple()));
    let file =
        create_new_private(&path).map_err(|error| format!("Could not create the temporary audio copy: {error}"))?;
    Ok((TempFile::new(path), file))
}

/// A new, empty temporary file for another program (FFmpeg, the WAV writer)
/// to write into; it keeps the 0600 mode when they truncate it.
pub(crate) fn new_temp_path(prefix: &str, ext: &str) -> Result<TempFile, String> {
    new_temp_file(prefix, ext).map(|(guard, _file)| guard)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Where test runs' folders are.
    pub(crate) fn test_root() -> &'static Path {
        root()
    }

    /// The names in this test thread's run folder.
    pub(crate) fn thread_run_files() -> Vec<String> {
        let dir = run_dir().unwrap();
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!("depoaudio_workdir_{label}_{}", Uuid::new_v4().simple()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_run_is_private_and_locked() {
        let base = TestDir::new("perms");
        let root = base.0.join("work");
        let run = start_run(&root).unwrap();
        assert!(run.dir().is_dir());
        assert_eq!(run.dir().parent().unwrap(), root);
        let lock = root.join(format!("{}.lock", run.id));
        assert!(lock.is_file());
        #[cfg(unix)]
        {
            assert_eq!(mode(&root), 0o700);
            assert_eq!(mode(run.dir()), 0o700);
            assert_eq!(mode(&lock), 0o600);
        }
        // Another handle cannot take the lock while the run holds it.
        let other = File::open(&lock).unwrap();
        assert!(other.try_lock().is_err());
    }

    #[test]
    fn temporary_files_are_new_private_and_removed_with_their_guard() {
        let (guard, mut file) = new_temp_file("depoaudio_", "wav").unwrap();
        use std::io::Write;
        file.write_all(b"RIFF").unwrap();
        drop(file);
        assert!(guard.path.starts_with(root()));
        assert_eq!(thread_run_files().len(), 1);
        assert_eq!(guard.path.extension().unwrap(), "wav");
        #[cfg(unix)]
        assert_eq!(mode(&guard.path), 0o600);
        // create_new: the same name cannot be opened again as new.
        assert!(create_new_private(&guard.path).is_err());
        let path = guard.path.clone();
        drop(guard);
        assert!(!path.exists());
        assert!(thread_run_files().is_empty());
    }

    #[test]
    fn sweep_removes_free_runs_and_skips_locked_ones() {
        let base = TestDir::new("sweep");
        let root = base.0.join("work");
        let legacy = base.0.join("tmp");
        fs::create_dir_all(&legacy).unwrap();

        let held = start_run(&root).unwrap();
        fs::write(held.dir().join("depoaudio_held.wav"), b"x").unwrap();
        // A run whose app has exited: its lock file is there but free.
        let gone = start_run(&root).unwrap();
        let gone_id = gone.id.clone();
        let gone_dir = gone.dir().to_path_buf();
        fs::write(gone_dir.join("depoaudio_gone.wav"), b"x").unwrap();
        drop(gone);
        // A folder left without its lock file.
        let orphan = root.join(Uuid::new_v4().simple().to_string());
        fs::create_dir_all(&orphan).unwrap();
        // Unrelated names are never touched.
        let other = root.join("notes.txt");
        fs::write(&other, b"keep").unwrap();
        let old_copy = legacy.join(format!("depoaudio_{}.wav", Uuid::new_v4().simple()));
        fs::write(&old_copy, b"x").unwrap();

        // Another instance (`held`) holds a lock: old copies are kept.
        let report = sweep(&root, None, Some(&legacy));
        assert_eq!(report.removed_runs, 2, "{report:?}");
        assert_eq!(report.locked_runs, 1);
        assert_eq!(report.removed_legacy, 0);
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(!gone_dir.exists());
        assert!(!root.join(format!("{gone_id}.lock")).exists());
        assert!(!orphan.exists());
        assert!(held.dir().join("depoaudio_held.wav").exists());
        assert!(other.exists());
        assert!(old_copy.exists());

        // The sweeping run's own lock does not count as another instance.
        let report = sweep(&root, Some(&held.id), Some(&legacy));
        assert_eq!(report.locked_runs, 0);
        assert_eq!(report.removed_legacy, 1);
        assert!(!old_copy.exists());
        assert!(held.dir().exists());

        // Once the holder exits, its folder goes too.
        let held_dir = held.dir().to_path_buf();
        drop(held);
        let report = sweep(&root, None, Some(&legacy));
        assert_eq!(report.removed_runs, 1);
        assert!(!held_dir.exists());
        assert!(other.exists());
    }

    #[test]
    fn only_old_depoaudio_audio_copy_names_are_swept() {
        let id = Uuid::new_v4().simple().to_string();
        for name in [
            format!("depoaudio_{id}.wav"),
            format!("depoaudio_{id}.sgmca"),
            format!("depoaudio_merged_{id}.wav"),
            format!("depoaudio_merge_{id}.wav"),
        ] {
            assert!(is_legacy_temp_name(&name), "{name}");
        }
        for name in [
            "depoaudio_notes.wav".to_string(),
            format!("depoaudio_{id}.txt"),
            format!("depoaudio_merge_{id}.sgmca"),
            format!("recording_{id}.wav"),
            format!("depoaudio_pcm_limit_{id}.wav"),
            format!("depoaudio_{id}"),
        ] {
            assert!(!is_legacy_temp_name(&name), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn sweep_removes_a_link_and_never_follows_it() {
        let base = TestDir::new("link");
        let root = base.0.join("work");
        fs::create_dir_all(&root).unwrap();
        let outside = base.0.join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("user.wav"), b"keep").unwrap();
        let id = Uuid::new_v4().simple().to_string();
        std::os::unix::fs::symlink(&outside, root.join(&id)).unwrap();
        let report = sweep(&root, None, None);
        assert_eq!(report.removed_runs, 1);
        assert!(outside.join("user.wav").exists());
    }
}
