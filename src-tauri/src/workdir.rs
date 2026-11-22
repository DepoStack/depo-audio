//! DepoAudio's private work folder for temporary audio copies.
//!
//! Every temporary audio file (the copy of a prefixed SGMCA, the WAV read out
//! of a `.dm`, an FTR recording's split channel files, Merge's decoded and
//! mixed WAVs) is created in `<app cache dir>/work/<run-id>/`, one run folder
//! per app launch (the FTR channel files in a 0700 folder of their own per
//! conversion inside it, see [`new_temp_dir`]):
//!
//! - On Unix the `work` and run folders are mode 0700 and each file is created
//!   only if new, with mode 0600. On Windows the per-user cache folder's
//!   access rules apply; nothing extra is set.
//! - Each run holds `work/<run-id>.lock` locked (std `File::try_lock`) for as
//!   long as the app runs. The lock file is created under a temporary name,
//!   locked, and only then renamed to `<run-id>.lock`, so a sweep never finds
//!   a new run's lock file unlocked. A sweep removes a lock file only while it
//!   holds that lock itself. If the lock file disappears while the app runs
//!   (for example the cache folder was cleaned), it is made and locked again
//!   the next time a temporary file is created.
//! - A run folder whose lock is free belongs to no running DepoAudio and is
//!   removed by the sweep at launch and by "Clear all DepoAudio data". A
//!   locked one is left alone. A run folder with no lock file at all is
//!   removed only once nothing in it has changed for [`STALE_AFTER`]. A lock
//!   that cannot be opened or checked is never removed; it is reported.
//! - If the file system cannot lock files, the run goes ahead without a lock
//!   (a message is logged). Its lock file is still made, so other DepoAudio
//!   windows, which cannot check it either, leave its folder alone.
//! - Old `depoaudio_*` audio copies that earlier versions left in the system
//!   temporary folder are removed by the same sweep only when no other
//!   DepoAudio holds a run lock, the copy has not changed for
//!   [`STALE_AFTER`] (an older DepoAudio that is still open takes no lock, so
//!   age is what protects its copies), and, on Unix, the copy belongs to the
//!   same user as the work folder.
//! - Temporary files are not `sync_all`ed: they are read back by this run only.
//! - On macOS the app cache folder is under `~/Library/Caches`, which Time
//!   Machine's standard exclusions already skip; no extra attribute is set.
//!   (Not checked on a Mac in this change. Removing a held lock file and
//!   renaming a locked one are not checked on Windows in this change either.)

use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use uuid::Uuid;

use crate::safety::TempFile;

/// Name of the folder under the app cache folder.
const WORK_FOLDER: &str = "work";
/// Suffix of a lock file that is being made and is not locked yet.
const NEW_LOCK_SUFFIX: &str = ".lock.new";
/// How long an unprotected item (a run folder with no lock file, an old
/// system-temp copy, a half-made lock file) must have been unchanged before a
/// sweep removes it.
pub(crate) const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// Where run folders live. Set once at launch from the app cache folder.
static ROOT: OnceLock<PathBuf> = OnceLock::new();
/// This process's run: its folder and the held lock.
static RUN: Mutex<Option<Run>> = Mutex::new(None);

#[derive(Debug)]
pub(crate) struct Run {
    id: String,
    dir: PathBuf,
    lock_path: PathBuf,
    /// Held (locked) for the life of the run, unless the file system cannot
    /// lock files; the lock is released when the process exits, however it
    /// exits.
    lock: File,
}

impl Run {
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }

    /// Make sure the lock file is still there and is the one this run holds;
    /// if something outside the app removed it (or the whole work folder),
    /// make it and lock it again, then recreate the run folder.
    fn ensure_held(&mut self) -> Result<(), String> {
        if !names_this_file(&self.lock, &self.lock_path) {
            let root = self.dir.parent().map(Path::to_path_buf).unwrap_or_default();
            ensure_private_dir(&root).map_err(work_folder_error)?;
            self.lock = publish_lock(&root, &self.id).map_err(work_folder_error)?;
        }
        ensure_private_dir(&self.dir).map_err(work_folder_error)
    }
}

/// What a sweep did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SweepReport {
    /// Run folders removed (their lock was free, or they had no lock file and
    /// were stale).
    pub removed_runs: usize,
    /// Run folders left because another DepoAudio holds their lock.
    pub locked_runs: usize,
    /// Run folders left because they have no lock file but changed recently.
    pub recent_unlocked_runs: usize,
    /// Old `depoaudio_*` files removed from the system temporary folder.
    pub removed_legacy: usize,
    /// (name, reason) for each item that could not be removed or checked.
    pub failed: Vec<(String, String)>,
}

fn is_run_id(name: &str) -> bool {
    name.len() == 32 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn work_folder_error(error: std::io::Error) -> String {
    format!("Could not create DepoAudio's private work folder: {error}")
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

/// Does `path` still name the open `file`? On Unix the device and inode are
/// compared. Elsewhere only that the name exists: a sweep removes a lock file
/// only while holding its lock, so once this run holds the lock, a lock file
/// still at that name is this run's.
fn names_this_file(file: &File, path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (file.metadata(), fs::metadata(path)) {
            (Ok(open), Ok(named)) => open.dev() == named.dev() && open.ino() == named.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = file;
        path.exists()
    }
}

/// Make `<root>/<id>.lock`: create it under a temporary name, lock it, then
/// rename it into place, so it is never visible to a sweep unlocked. The
/// handle is unlocked only when the file system cannot lock files.
fn publish_lock(root: &Path, id: &str) -> std::io::Result<File> {
    let new_path = root.join(format!("{id}{NEW_LOCK_SUFFIX}"));
    let lock_path = root.join(format!("{id}.lock"));
    let lock = create_new_private(&new_path)?;
    let locked = match lock.try_lock() {
        Ok(()) => true,
        Err(TryLockError::Error(error)) => {
            eprintln!("[workdir] This folder cannot lock files ({error}); the run's temporary folder is not locked.");
            false
        }
        Err(TryLockError::WouldBlock) => {
            // Only a stale-file sweep could hold it, and it removes it.
            drop(lock);
            let _ = fs::remove_file(&new_path);
            return Err(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "the new lock file was taken by a sweep",
            ));
        }
    };
    if let Err(error) = fs::rename(&new_path, &lock_path) {
        drop(lock);
        let _ = fs::remove_file(&new_path);
        return Err(error);
    }
    if locked && !names_this_file(&lock, &lock_path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the lock file was removed as it was made",
        ));
    }
    Ok(lock)
}

/// Start a new run under `root`: make and lock `<id>.lock`, then the `<id>/`
/// folder. The lock exists, held, before the folder, so a sweep never sees
/// this run's folder without its held lock.
pub(crate) fn start_run(root: &Path) -> Result<Run, String> {
    ensure_private_dir(root).map_err(work_folder_error)?;
    let mut last_error = None;
    for _ in 0..4 {
        let id = Uuid::new_v4().simple().to_string();
        let lock = match publish_lock(root, &id) {
            Ok(published) => published,
            Err(error) => {
                last_error = Some(error);
                continue;
            }
        };
        let dir = root.join(&id);
        ensure_private_dir(&dir).map_err(work_folder_error)?;
        let lock_path = root.join(format!("{id}.lock"));
        return Ok(Run {
            id,
            dir,
            lock_path,
            lock,
        });
    }
    Err(match last_error {
        Some(error) => work_folder_error(error),
        None => "Could not create DepoAudio's private work folder".into(),
    })
}

/// The state of `<root>/<id>.lock`.
enum LockState {
    /// Free; now held by this handle.
    Free(File),
    /// Held by a running DepoAudio.
    Held,
    /// There is no lock file.
    Missing,
    /// It could not be opened or checked; never remove what cannot be checked.
    Unchecked(std::io::Error),
}

fn take_free_lock(lock_path: &Path) -> LockState {
    match OpenOptions::new().read(true).write(true).open(lock_path) {
        Ok(file) => match file.try_lock() {
            Ok(()) => LockState::Free(file),
            Err(TryLockError::WouldBlock) => LockState::Held,
            Err(TryLockError::Error(error)) => LockState::Unchecked(error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LockState::Missing,
        Err(error) => LockState::Unchecked(error),
    }
}

/// Has `meta`'s modification time been at least `stale_after` ago? A time in
/// the future, or one that cannot be read, counts as recent.
fn is_stale(meta: &fs::Metadata, now: SystemTime, stale_after: Duration) -> bool {
    meta.modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age >= stale_after)
}

/// Is a run folder (itself and the items directly in it) unchanged for
/// `stale_after`?
fn run_dir_is_stale(dir: &Path, now: SystemTime, stale_after: Duration) -> bool {
    let Ok(meta) = fs::symlink_metadata(dir) else {
        return true;
    };
    if !is_stale(&meta, now, stale_after) {
        return false;
    }
    if !meta.is_dir() {
        return true;
    }
    match fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .all(|entry| fs::symlink_metadata(entry.path()).is_ok_and(|meta| is_stale(&meta, now, stale_after))),
        Err(_) => false,
    }
}

/// Remove a run folder, or a link or file in its place; a link is removed as
/// a link, never followed.
fn remove_run_dir(dir: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(dir),
        Ok(_) => fs::remove_file(dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
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

/// Remove every run folder under `root` whose lock is free, except `own`, and
/// run folders with no lock file that have been unchanged for `stale_after`.
/// When no other run's lock is held, also remove old `depoaudio_*` audio
/// copies directly in `legacy_dir` that have been unchanged for
/// `stale_after` and (on Unix) have the same owner as `root`. Only names
/// DepoAudio makes are touched; symlinks are removed as links, never followed.
pub(crate) fn sweep(root: &Path, own: Option<&str>, legacy_dir: Option<&Path>, stale_after: Duration) -> SweepReport {
    let now = SystemTime::now();
    let mut report = SweepReport::default();
    let mut ids = std::collections::BTreeSet::new();
    let mut new_locks = Vec::new();
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(id) = name.strip_suffix(NEW_LOCK_SUFFIX) {
                if is_run_id(id) && Some(id) != own {
                    new_locks.push(entry.path());
                }
                continue;
            }
            let id = name.strip_suffix(".lock").unwrap_or(&name);
            if is_run_id(id) && Some(id) != own {
                ids.insert(id.to_string());
            }
        }
    }
    for id in ids {
        let lock_path = root.join(format!("{id}.lock"));
        let dir = root.join(&id);
        let lock = match take_free_lock(&lock_path) {
            LockState::Free(lock) => Some(lock),
            LockState::Missing => {
                if !run_dir_is_stale(&dir, now, stale_after) {
                    report.recent_unlocked_runs += 1;
                    continue;
                }
                None
            }
            LockState::Held => {
                report.locked_runs += 1;
                continue;
            }
            LockState::Unchecked(error) => {
                report.failed.push((
                    format!("work/{id}"),
                    format!("its lock could not be checked: {}", error.kind()),
                ));
                continue;
            }
        };
        match remove_run_dir(&dir) {
            Ok(()) => report.removed_runs += 1,
            Err(error) => {
                report.failed.push((format!("work/{id}"), error.kind().to_string()));
                continue;
            }
        }
        if let Some(lock) = lock {
            // Removed while still held, so a run that is being made can
            // never take the lock of a name that is about to go.
            if let Err(error) = fs::remove_file(&lock_path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    report
                        .failed
                        .push((format!("work/{id}.lock"), error.kind().to_string()));
                }
            }
            drop(lock);
        }
    }
    // Half-made lock files from a run that stopped while starting.
    for path in new_locks {
        let stale = fs::symlink_metadata(&path).is_ok_and(|meta| is_stale(&meta, now, stale_after));
        if !stale {
            continue;
        }
        if let LockState::Free(lock) = take_free_lock(&path) {
            let _ = fs::remove_file(&path);
            drop(lock);
        }
    }

    if report.locked_runs == 0 {
        if let Some(legacy_dir) = legacy_dir {
            sweep_legacy(root, legacy_dir, now, stale_after, &mut report);
        }
    }
    report
}

fn sweep_legacy(root: &Path, legacy_dir: &Path, now: SystemTime, stale_after: Duration, report: &mut SweepReport) {
    #[cfg(unix)]
    let owner = {
        use std::os::unix::fs::MetadataExt;
        // Without a work folder to compare with, nothing is removed.
        match fs::metadata(root) {
            Ok(meta) => meta.uid(),
            Err(_) => return,
        }
    };
    #[cfg(not(unix))]
    let _ = root;
    let Ok(entries) = fs::read_dir(legacy_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_legacy_temp_name(&name) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        // A regular file or a link; never a folder.
        if meta.is_dir() || !is_stale(&meta, now, stale_after) {
            continue;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            // Another account's copy in a shared temporary folder.
            if meta.uid() != owner {
                continue;
            }
        }
        match fs::remove_file(&path) {
            Ok(()) => report.removed_legacy += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => report.failed.push((name, error.kind().to_string())),
        }
    }
}

fn default_root() -> PathBuf {
    #[cfg(test)]
    {
        // Shared by test processes; leftovers of finished ones are swept
        // the first time a test process uses it.
        let root = std::env::temp_dir().join("depoaudio_test_work");
        sweep(&root, None, None, STALE_AFTER);
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

/// For "Clear all DepoAudio data": the same sweep, keeping this run. It can
/// take a while on a slow disk; call it off the main thread and without
/// holding other locks.
pub(crate) fn sweep_now() -> SweepReport {
    let own = RUN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(|run| run.id.clone());
    sweep(root(), own.as_deref(), Some(&std::env::temp_dir()), STALE_AFTER)
}

#[cfg(test)]
struct ThreadRun(Run);

#[cfg(test)]
impl Drop for ThreadRun {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0.dir);
        let _ = fs::remove_file(&self.0.lock_path);
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
        let run = run.as_mut().expect("started above");
        run.0.ensure_held()?;
        Ok(run.0.dir().to_path_buf())
    })
}

/// This run's folder, starting the run if launch could not.
#[cfg(not(test))]
fn run_dir() -> Result<PathBuf, String> {
    let mut run = RUN.lock().unwrap_or_else(|error| error.into_inner());
    if run.is_none() {
        *run = Some(start_run(root())?);
    }
    let run = run.as_mut().expect("started above");
    // Recreate the lock file and folder if something outside the app removed
    // them.
    run.ensure_held()?;
    Ok(run.dir().to_path_buf())
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

/// A temporary folder in this run's folder, removed with everything in it
/// when dropped.
#[derive(Debug)]
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // The folder is this run's own (random name, created here, owner
        // only); remove_dir_all does not follow links inside it.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A new, empty folder in this run's folder, named `<prefix><uuid>`, for
/// several temporary files that belong together (an FTR recording's channel
/// files). It must not exist yet (an existing folder or link is never
/// reused) and on Unix is mode 0700, so another local user can neither read
/// the decoded audio nor replace a file in it before FFmpeg reads it. A
/// launch or "Clear all DepoAudio data" sweep removes it with its run folder
/// if the run was killed before the guard dropped.
pub(crate) fn new_temp_dir(prefix: &str) -> Result<TempDir, String> {
    let dir = run_dir()?;
    new_temp_dir_in(&dir, prefix)
}

fn new_temp_dir_in(parent: &Path, prefix: &str) -> Result<TempDir, String> {
    let fail = |error: std::io::Error| format!("Could not create a temporary folder for audio files: {error}");
    let path = parent.join(format!("{prefix}{}", Uuid::new_v4().simple()));
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&path).map_err(fail)?;
    let guard = TempDir(path);
    let metadata = fs::symlink_metadata(guard.path()).map_err(fail)?;
    if !metadata.is_dir() {
        return Err("The temporary folder for audio files is not a plain folder.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err("The temporary folder for audio files could be read by other users.".into());
        }
    }
    Ok(guard)
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
    fn temporary_folders_are_new_private_and_removed_with_everything_in_them() {
        let made = new_temp_dir("depoaudio_trm_run_").unwrap();
        assert!(made.path().is_dir());
        assert_eq!(made.path().parent(), Some(run_dir().unwrap().as_path()));
        #[cfg(unix)]
        assert_eq!(mode(made.path()), 0o700);
        // Creation never accepts a folder that is already there.
        assert!(fs::DirBuilder::new().create(made.path()).is_err());
        fs::write(made.path().join("depoaudio_trm_ch0.aac"), b"x").unwrap();
        let other = new_temp_dir("depoaudio_trm_run_").unwrap();
        assert_ne!(other.path(), made.path());
        let path = made.path().to_path_buf();
        drop(made);
        drop(other);
        assert!(!path.exists());
        assert!(thread_run_files().is_empty());
        // A missing parent is an error, not a folder made elsewhere.
        let base = TestDir::new("tempdir");
        assert!(new_temp_dir_in(&base.0.join("missing"), "depoaudio_trm_run_").is_err());
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

    /// Set a file's modification time `age` into the past.
    fn backdate(path: &Path, age: Duration) {
        let file = OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    const OLD: Duration = Duration::from_secs(2 * 24 * 60 * 60);

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
        // Unrelated names are never touched.
        let other = root.join("notes.txt");
        fs::write(&other, b"keep").unwrap();
        let old_copy = legacy.join(format!("depoaudio_{}.wav", Uuid::new_v4().simple()));
        fs::write(&old_copy, b"x").unwrap();
        backdate(&old_copy, OLD);

        // Another instance (`held`) holds a lock: old copies are kept.
        let report = sweep(&root, None, Some(&legacy), STALE_AFTER);
        assert_eq!(report.removed_runs, 1, "{report:?}");
        assert_eq!(report.locked_runs, 1);
        assert_eq!(report.removed_legacy, 0);
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(!gone_dir.exists());
        assert!(!root.join(format!("{gone_id}.lock")).exists());
        assert!(held.dir().join("depoaudio_held.wav").exists());
        assert!(other.exists());
        assert!(old_copy.exists());

        // The sweeping run's own lock does not count as another instance.
        let report = sweep(&root, Some(&held.id), Some(&legacy), STALE_AFTER);
        assert_eq!(report.locked_runs, 0);
        assert_eq!(report.removed_legacy, 1);
        assert!(!old_copy.exists());
        assert!(held.dir().exists());

        // Once the holder exits, its folder goes too.
        let held_dir = held.dir().to_path_buf();
        drop(held);
        let report = sweep(&root, None, Some(&legacy), STALE_AFTER);
        assert_eq!(report.removed_runs, 1);
        assert!(!held_dir.exists());
        assert!(other.exists());
    }

    #[test]
    fn a_run_folder_without_a_lock_file_is_removed_only_when_stale() {
        let base = TestDir::new("lockless");
        let root = base.0.join("work");
        let orphan = root.join(Uuid::new_v4().simple().to_string());
        fs::create_dir_all(&orphan).unwrap();
        fs::write(orphan.join("depoaudio_x.wav"), b"x").unwrap();

        let report = sweep(&root, None, None, STALE_AFTER);
        assert_eq!(report.removed_runs, 0, "{report:?}");
        assert_eq!(report.recent_unlocked_runs, 1);
        assert!(orphan.join("depoaudio_x.wav").exists());

        let report = sweep(&root, None, None, Duration::ZERO);
        assert_eq!(report.removed_runs, 1, "{report:?}");
        assert!(!orphan.exists());
    }

    #[test]
    fn a_lock_that_cannot_be_checked_is_kept_and_reported() {
        let base = TestDir::new("unchecked");
        let root = base.0.join("work");
        let legacy = base.0.join("tmp");
        fs::create_dir_all(&legacy).unwrap();
        let id = Uuid::new_v4().simple().to_string();
        fs::create_dir_all(root.join(&id)).unwrap();
        // A folder where the lock file should be cannot be opened as one.
        fs::create_dir_all(root.join(format!("{id}.lock"))).unwrap();
        let old_copy = legacy.join(format!("depoaudio_{}.wav", Uuid::new_v4().simple()));
        fs::write(&old_copy, b"x").unwrap();
        backdate(&old_copy, OLD);

        let report = sweep(&root, None, Some(&legacy), Duration::ZERO);
        assert_eq!(report.removed_runs, 0, "{report:?}");
        assert!(root.join(&id).exists());
        assert_eq!(report.failed.len(), 1, "{report:?}");
        assert_eq!(report.failed[0].0, format!("work/{id}"));
        assert!(report.failed[0].1.starts_with("its lock could not be checked"));
        // It does not stop the old copies from being swept.
        assert_eq!(report.removed_legacy, 1);
    }

    #[test]
    fn a_run_whose_lock_file_was_removed_locks_it_again() {
        let base = TestDir::new("relock");
        let root = base.0.join("work");
        let mut run = start_run(&root).unwrap();
        fs::write(run.dir().join("depoaudio_live.wav"), b"x").unwrap();
        fs::remove_file(&run.lock_path).unwrap();

        run.ensure_held().unwrap();
        assert!(names_this_file(&run.lock, &run.lock_path));
        let report = sweep(&root, None, None, Duration::ZERO);
        assert_eq!(report.locked_runs, 1, "{report:?}");
        assert_eq!(report.removed_runs, 0);
        assert!(run.dir().join("depoaudio_live.wav").exists());

        // The whole work folder removed (a cache cleaner): both come back.
        fs::remove_dir_all(&root).unwrap();
        run.ensure_held().unwrap();
        assert!(run.dir().is_dir());
        assert!(names_this_file(&run.lock, &run.lock_path));
        #[cfg(unix)]
        assert_eq!(mode(&root), 0o700);
        let report = sweep(&root, None, None, Duration::ZERO);
        assert_eq!(report.locked_runs, 1, "{report:?}");
        assert!(run.dir().is_dir());
    }

    #[test]
    fn stale_half_made_lock_files_are_removed() {
        let base = TestDir::new("newlock");
        let root = base.0.join("work");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("{}{NEW_LOCK_SUFFIX}", Uuid::new_v4().simple()));
        fs::write(&path, b"").unwrap();
        sweep(&root, None, None, STALE_AFTER);
        assert!(path.exists());
        sweep(&root, None, None, Duration::ZERO);
        assert!(!path.exists());
    }

    #[test]
    fn only_stale_old_copies_are_swept() {
        let base = TestDir::new("legacyage");
        let root = base.0.join("work");
        fs::create_dir_all(&root).unwrap();
        let legacy = base.0.join("tmp");
        fs::create_dir_all(&legacy).unwrap();
        // An older DepoAudio that is still open may be using this one.
        let recent = legacy.join(format!("depoaudio_{}.sgmca", Uuid::new_v4().simple()));
        fs::write(&recent, b"x").unwrap();
        let old = legacy.join(format!("depoaudio_merged_{}.wav", Uuid::new_v4().simple()));
        fs::write(&old, b"x").unwrap();
        backdate(&old, OLD);
        let report = sweep(&root, None, Some(&legacy), STALE_AFTER);
        assert_eq!(report.removed_legacy, 1, "{report:?}");
        assert!(recent.exists());
        assert!(!old.exists());
    }

    /// Another account's copy in a shared temporary folder is left alone and
    /// not reported. Needs permission to change a file's owner (root); the
    /// check is skipped otherwise.
    #[cfg(unix)]
    #[test]
    fn another_users_old_copies_are_left_alone() {
        let base = TestDir::new("legacyowner");
        let root = base.0.join("work");
        fs::create_dir_all(&root).unwrap();
        let legacy = base.0.join("tmp");
        fs::create_dir_all(&legacy).unwrap();
        let theirs = legacy.join(format!("depoaudio_{}.wav", Uuid::new_v4().simple()));
        fs::write(&theirs, b"x").unwrap();
        backdate(&theirs, OLD);
        if std::os::unix::fs::chown(&theirs, Some(65534), None).is_err() {
            eprintln!("skipped: cannot change a file's owner here");
            return;
        }
        let report = sweep(&root, None, Some(&legacy), STALE_AFTER);
        assert_eq!(report.removed_legacy, 0, "{report:?}");
        assert!(report.failed.is_empty(), "{report:?}");
        assert!(theirs.exists());
    }

    /// Sweeps from other windows running while runs start never leave a
    /// started run without its held lock file or folder.
    #[test]
    fn runs_started_during_sweeps_keep_their_lock_and_folder() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let base = TestDir::new("race");
        let root = base.0.join("work");
        fs::create_dir_all(&root).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let sweepers: Vec<_> = (0..3)
            .map(|_| {
                let root = root.clone();
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        sweep(&root, None, None, STALE_AFTER);
                    }
                })
            })
            .collect();
        let mut problems = Vec::new();
        for _ in 0..3000 {
            let run = match start_run(&root) {
                Ok(run) => run,
                Err(error) => {
                    problems.push(error);
                    continue;
                }
            };
            // Let the sweepers pass over it at least once.
            sweep(&root, Some(&run.id), None, STALE_AFTER);
            if !names_this_file(&run.lock, &run.lock_path) {
                problems.push(format!("{}: lock file gone", run.id));
            }
            if !run.dir().is_dir() {
                problems.push(format!("{}: folder gone", run.id));
            }
            let id = run.id.clone();
            drop(run);
            let _ = remove_run_dir(&root.join(&id));
            let _ = fs::remove_file(root.join(format!("{id}.lock")));
        }
        stop.store(true, Ordering::Relaxed);
        for sweeper in sweepers {
            sweeper.join().unwrap();
        }
        assert!(
            problems.is_empty(),
            "{} problem(s): {:?}",
            problems.len(),
            &problems[..problems.len().min(5)]
        );
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
        let report = sweep(&root, None, None, Duration::ZERO);
        assert_eq!(report.removed_runs, 1);
        assert!(outside.join("user.wav").exists());
    }
}
