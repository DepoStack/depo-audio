use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// Owns one app-data store until the process drops its last handle or exits.
/// The lock file is permanent: unlinking it could let another process lock a
/// different inode while the current owner still holds the original one.
pub(crate) struct StoreOwner {
    _file: File,
}

impl StoreOwner {
    pub(crate) fn acquire(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let path = directory.join(".store-owner.lock");
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(io::Error::new(io::ErrorKind::InvalidInput, "Store lock must be a regular file"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let file = open_locked(&path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "Store lock must be a regular file"));
        }
        Ok(Self { _file: file })
    }
}

#[cfg(target_os = "windows")]
fn open_locked(path: &Path) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;

    // No sharing includes deletion/rename, so another instance cannot replace
    // the lock path while this handle is alive. Closing the handle releases it.
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn open_locked(path: &Path) -> io::Result<File> {
    use std::os::fd::AsRawFd;
    use std::os::raw::c_int;
    use std::os::unix::fs::OpenOptionsExt;

    // File::try_lock requires Rust 1.89; keep the declared 1.88 MSRV. These
    // flock constants and ABI are shared by Linux and macOS (sys/file.h).
    const LOCK_EX: c_int = 2;
    const LOCK_NB: c_int = 4;
    unsafe extern "C" {
        fn flock(fd: c_int, operation: c_int) -> c_int;
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    // SAFETY: `file` owns a valid live descriptor for the duration of this
    // call. flock takes only this descriptor and the documented flag bits; it
    // does not retain pointers or take ownership. File drop releases the lock.
    if unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(file)
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn open_locked(_path: &Path) -> io::Result<File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Exclusive app-data ownership is not supported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct TestDirectory(std::path::PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("depoaudio_owner_{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct TestChild(Child);

    impl Drop for TestChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn child_command(dir: &std::path::Path, hold: bool) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "store_owner::tests::ownership_subprocess", "--ignored", "--nocapture"])
            .env("DEPOAUDIO_OWNER_TEST_DIRECTORY", dir)
            .env("DEPOAUDIO_OWNER_TEST_HOLD", if hold { "1" } else { "0" })
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    #[test]
    fn another_owner_is_excluded_until_the_first_handle_drops() {
        let dir = TestDirectory::new();
        let owner = StoreOwner::acquire(&dir.0).unwrap();
        assert!(StoreOwner::acquire(&dir.0).is_err());
        drop(owner);
        assert!(dir.0.join(".store-owner.lock").is_file());
        let _next_owner = StoreOwner::acquire(&dir.0).unwrap();
    }

    #[test]
    fn independent_stores_can_have_independent_owners() {
        let first = TestDirectory::new();
        let second = TestDirectory::new();
        let _first_owner = StoreOwner::acquire(&first.0).unwrap();
        let _second_owner = StoreOwner::acquire(&second.0).unwrap();
    }

    #[test]
    fn an_existing_unlocked_file_is_not_a_stale_ownership_marker() {
        let dir = TestDirectory::new();
        let path = dir.0.join(".store-owner.lock");
        std::fs::write(&path, b"existing file").unwrap();
        let owner = StoreOwner::acquire(&dir.0).unwrap();
        drop(owner);
        assert_eq!(std::fs::metadata(path).unwrap().len(), 13);
    }

    #[test]
    fn non_file_lock_target_is_rejected() {
        let dir = TestDirectory::new();
        std::fs::create_dir(dir.0.join(".store-owner.lock")).unwrap();
        assert!(StoreOwner::acquire(&dir.0).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlink_lock_target_is_rejected() {
        let dir = TestDirectory::new();
        let target = dir.0.join("other-file");
        std::fs::write(&target, b"untouched").unwrap();
        std::os::unix::fs::symlink(&target, dir.0.join(".store-owner.lock")).unwrap();
        assert!(StoreOwner::acquire(&dir.0).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"untouched");
    }

    #[test]
    fn a_separate_process_cannot_acquire_an_owned_store() {
        let dir = TestDirectory::new();
        let owner = StoreOwner::acquire(&dir.0).unwrap();
        assert_eq!(child_command(&dir.0, false).status().unwrap().code(), Some(73));
        drop(owner);
        assert!(child_command(&dir.0, false).status().unwrap().success());
    }

    #[test]
    fn terminated_process_releases_ownership_without_deleting_the_lock_file() {
        let dir = TestDirectory::new();
        let mut child = TestChild(child_command(&dir.0, true).spawn().unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !dir.0.join("ready").exists() {
            assert!(child.0.try_wait().unwrap().is_none(), "owner child exited before becoming ready");
            assert!(Instant::now() < deadline, "owner child did not become ready");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(StoreOwner::acquire(&dir.0).is_err());
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert!(dir.0.join(".store-owner.lock").is_file());
        let _next_owner = StoreOwner::acquire(&dir.0).unwrap();
    }

    #[test]
    #[ignore = "invoked only as a subprocess by ownership tests"]
    fn ownership_subprocess() {
        let dir = std::path::PathBuf::from(std::env::var_os("DEPOAUDIO_OWNER_TEST_DIRECTORY").unwrap());
        let owner = match StoreOwner::acquire(&dir) {
            Ok(owner) => owner,
            Err(_) => std::process::exit(73),
        };
        if std::env::var("DEPOAUDIO_OWNER_TEST_HOLD").as_deref() == Ok("1") {
            std::fs::write(dir.join("ready"), b"ready").unwrap();
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
        }
        drop(owner);
    }
}
