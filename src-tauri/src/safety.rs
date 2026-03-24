use std::path::{Path, PathBuf};

// ── Safety checks ───────────────────────────────────────────────────────────
//
// Guards against resource exhaustion and path manipulation.

/// Maximum audio file size allowed for in-memory processing (2 GB).
const MAX_AUDIO_SIZE: u64 = 2 * 1024 * 1024 * 1024;

/// Check that a file exists and is within the size limit before processing.
pub(crate) fn check_file_safe(path: &Path) -> Result<(), String> {
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

    if size > MAX_AUDIO_SIZE {
        return Err(format!(
            "File is too large ({:.1} GB). Maximum supported size is 2 GB.",
            size as f64 / (1024.0 * 1024.0 * 1024.0)
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
