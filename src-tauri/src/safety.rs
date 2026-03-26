use std::path::{Path, PathBuf};

// ── Safety checks ───────────────────────────────────────────────────────────
//
// Guards against resource exhaustion and path manipulation.

/// Maximum audio file size allowed for in-memory processing (2 GB).
const MAX_AUDIO_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Check that a file exists and is within the size limit before processing.
pub(crate) fn check_file_safe(path: &Path) -> Result<(), String> {
    check_file_safe_with_limit(path, MAX_AUDIO_SIZE)
}

/// Check with a custom size limit (in bytes).
pub(crate) fn check_file_safe_with_limit(path: &Path, max_size: u64) -> Result<(), String> {
    if !path.exists() {
        return Err("File not found".into());
    }
    if !path.is_file() {
        return Err("Not a file".into());
    }

    let size = std::fs::metadata(path)
        .map(|m| m.len())
        .unwrap_or(0);

    if size == 0 {
        return Err("File is empty".into());
    }

    if size > max_size {
        return Err(format!(
            "File is too large ({:.1} GB). Maximum supported size is {:.1} GB.",
            size as f64 / (1024.0 * 1024.0 * 1024.0),
            max_size as f64 / (1024.0 * 1024.0 * 1024.0),
        ));
    }

    Ok(())
}

/// Sanitize a path for use in error messages — return only the filename.
pub(crate) fn safe_display(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("(unknown)")
        .to_string()
}

/// Validate numeric parameters are within reasonable bounds.
pub(crate) fn validate_rate(rate: &str) -> Result<(), String> {
    let r: u32 = rate.parse().map_err(|_| "Invalid sample rate")?;
    if r < 8000 || r > 192000 {
        return Err(format!("Sample rate {} Hz is outside valid range (8000-192000)", r));
    }
    Ok(())
}

pub(crate) fn validate_fade_dur(dur: f64) -> Result<(), String> {
    if dur.is_nan() || dur.is_infinite() || dur < 0.0 || dur > 30.0 {
        return Err(format!("Fade duration {} is outside valid range (0-30s)", dur));
    }
    Ok(())
}

/// Guard that cleans up a temp file when dropped.
/// Use this instead of manual `fs::remove_file` to ensure cleanup.
pub(crate) struct TempFile {
    pub path: PathBuf,
}

impl TempFile {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if self.path.exists() {
            if let Err(e) = std::fs::remove_file(&self.path) {
                eprintln!("Warning: failed to clean up temp file {}: {}", safe_display(&self.path), e);
            }
        }
    }
}

impl std::ops::Deref for TempFile {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TempFile {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn validate_rate_accepts_valid() {
        assert!(validate_rate("8000").is_ok());
        assert!(validate_rate("44100").is_ok());
        assert!(validate_rate("48000").is_ok());
        assert!(validate_rate("192000").is_ok());
    }

    #[test]
    fn validate_rate_rejects_invalid() {
        assert!(validate_rate("0").is_err());
        assert!(validate_rate("7999").is_err());
        assert!(validate_rate("192001").is_err());
        assert!(validate_rate("abc").is_err());
        assert!(validate_rate("").is_err());
    }

    #[test]
    fn validate_fade_dur_accepts_valid() {
        assert!(validate_fade_dur(0.0).is_ok());
        assert!(validate_fade_dur(0.5).is_ok());
        assert!(validate_fade_dur(30.0).is_ok());
    }

    #[test]
    fn validate_fade_dur_rejects_invalid() {
        assert!(validate_fade_dur(-1.0).is_err());
        assert!(validate_fade_dur(31.0).is_err());
        assert!(validate_fade_dur(f64::NAN).is_err());
        assert!(validate_fade_dur(f64::INFINITY).is_err());
    }

    #[test]
    fn safe_display_extracts_filename() {
        assert_eq!(safe_display(Path::new("/tmp/audio.wav")), "audio.wav");
        assert_eq!(safe_display(Path::new("audio.wav")), "audio.wav");
    }

    #[test]
    fn check_file_safe_rejects_missing() {
        assert!(check_file_safe(Path::new("/nonexistent/file.wav")).is_err());
    }

    #[test]
    fn check_file_safe_accepts_valid_file() {
        let dir = std::env::temp_dir().join("depoaudio_test_safety");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("test.wav");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(b"RIFF test data").unwrap();
        assert!(check_file_safe(&path).is_ok());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn check_file_safe_rejects_empty() {
        let dir = std::env::temp_dir().join("depoaudio_test_safety_empty");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("empty.wav");
        std::fs::File::create(&path).unwrap();
        assert!(check_file_safe(&path).is_err());
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn temp_file_cleans_up_on_drop() {
        let path = std::env::temp_dir().join("depoaudio_test_tempfile.tmp");
        {
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(b"temp").unwrap();
            let _guard = TempFile::new(path.clone());
            assert!(path.exists());
        }
        assert!(!path.exists());
    }
}
