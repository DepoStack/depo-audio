use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use std::sync::OnceLock;

use regex::Regex;
use uuid::Uuid;

use crate::types::FormatInfo;

fn date_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[_\-]?\d{4}[_\-]\d{2}[_\-]\d{2}|[_\-]?\d{8}|[_\-]?\d{2}[_\-]\d{2}[_\-]\d{4}").unwrap())
}

// ── Format registry ───────────────────────────────────────────────────────────

pub(crate) fn get_formats() -> Vec<FormatInfo> {
    vec![
        // Standard formats — play and import natively, convert optionally
        FormatInfo { key: "wav".into(), name: "WAV · PCM Audio".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "mp3".into(), name: "MP3 Audio".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "flac".into(), name: "FLAC Lossless".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "m4a".into(), name: "M4A · AAC Audio".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "ogg".into(), name: "OGG · Opus Audio".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "wma".into(), name: "Windows Media Audio".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "aiff".into(), name: "AIFF Audio".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "caf".into(), name: "CAF · Apple Core Audio".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "amr".into(), name: "AMR Phone Audio".into(), vendor: "Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "video".into(), name: "Video Audio Track".into(), vendor: "MP4 / MOV / MKV / AVI / WebM".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None,
            note: Some("The audio track is extracted and converted; video is discarded.".into()) },
        // Court reporting formats — require conversion
        FormatInfo { key: "sgmca".into(), name: "Stenograph SGMCA".into(), vendor: "Case CATalyst".into(),
            status: "supported".into(), handler: "sgmca".into(), channels: Some("4".into()), note: None },
        FormatInfo { key: "ftr".into(), name: "FTR Recording".into(), vendor: "For The Record".into(),
            status: "experimental".into(), handler: "ftr".into(), channels: Some("4–16".into()),
            note: Some("FTR uses proprietary AAC codec tag 0x4180. Drop all .trm files for a session together.".into()) },
        FormatInfo { key: "bwf".into(), name: "Broadcast WAV".into(), vendor: "CourtSmart / Various".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "digitalcat".into(), name: "DigitalCAT Audio".into(), vendor: "Stenovations".into(),
            status: "experimental".into(), handler: "passthrough".into(), channels: None,
            note: Some("No public spec — conversion may fail. Please report results on GitHub.".into()) },
        FormatInfo { key: "aes".into(), name: "Eclipse AudioSync".into(), vendor: "Eclipse CAT".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some("AES-128 encrypted. Open in Eclipse → File → Export Audio → WAV first.".into()) },
        FormatInfo { key: "dcr".into(), name: "Liberty Court Recorder".into(), vendor: "High Criteria".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some("DCR files are proprietary. Open in Liberty → File → Export Audio → WAV first.".into()) },
    ]
}

/// Check if a file extension is a standard audio format (no conversion needed for basic use)
#[allow(dead_code)]
pub(crate) fn is_standard_format(ext: &str) -> bool {
    matches!(ext, "wav" | "mp3" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "wma" | "aif" | "aiff" | "caf" | "amr")
}

/// Input-codec args a decode must pass BEFORE `-i` for formats FFmpeg cannot
/// auto-detect. FTR's proprietary AAC codec tag 0x4180 needs a forced decoder;
/// the convert path has always done this (conversion.rs) but the scan-path
/// decodes did not — so every analysis pass failed or wedged on .trm files.
pub(crate) fn scan_input_codec_args(path: &Path) -> Vec<String> {
    match detect_format_for_path(&path.to_string_lossy()) {
        Some(f) if f.handler == "ftr" => vec!["-acodec".into(), "aac".into()],
        _ => Vec::new(),
    }
}

pub(crate) fn detect_format_for_path(path: &str) -> Option<FormatInfo> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let fmts = get_formats();
    match ext.as_str() {
        // Court formats
        "sgmca"         => fmts.into_iter().find(|f| f.key == "sgmca"),
        "trm" | "ftr"   => fmts.into_iter().find(|f| f.key == "ftr"),
        "aes"           => fmts.into_iter().find(|f| f.key == "aes"),
        "dm"            => fmts.into_iter().find(|f| f.key == "digitalcat"),
        "dcr"           => fmts.into_iter().find(|f| f.key == "dcr"),
        "bwf"           => fmts.into_iter().find(|f| f.key == "bwf"),
        // Standard formats
        "wav"           => fmts.into_iter().find(|f| f.key == "wav"),
        "mp3"           => fmts.into_iter().find(|f| f.key == "mp3"),
        "flac"          => fmts.into_iter().find(|f| f.key == "flac"),
        "m4a" | "aac"   => fmts.into_iter().find(|f| f.key == "m4a"),
        "ogg" | "opus"  => fmts.into_iter().find(|f| f.key == "ogg"),
        "wma"           => fmts.into_iter().find(|f| f.key == "wma"),
        "aif" | "aiff"  => fmts.into_iter().find(|f| f.key == "aiff"),
        "caf"           => fmts.into_iter().find(|f| f.key == "caf"),
        "amr" | "3ga"   => fmts.into_iter().find(|f| f.key == "amr"),
        // Video containers: FFmpeg extracts the audio track
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" | "3gp"
                        => fmts.into_iter().find(|f| f.key == "video"),
        _               => None,
    }
}

// ── Case name detection ───────────────────────────────────────────────────────

pub(crate) fn infer_case_name(filename: &str) -> String {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);

    let cleaned = date_regex().replace_all(stem, "");

    let spaced = cleaned.replace('_', " ");
    let words: Vec<&str> = spaced.split_whitespace().collect();
    if words.is_empty() {
        return stem.to_string();
    }
    words.join(" ")
}

/// Validate and sanitize a user-supplied case name for library import:
/// trimmed, ≤200 chars, with path separators and control characters removed.
pub(crate) fn sanitize_case_name(case_name: &str) -> Result<String, String> {
    let trimmed = case_name.trim().to_string();
    if trimmed.is_empty() { return Err("Case name cannot be empty".into()); }
    if trimmed.len() > 200 { return Err("Case name is too long (max 200 characters)".into()); }
    let sanitized: String = trimmed.chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\' && *c != ':')
        .collect();
    if sanitized.is_empty() { return Err("Case name contains only invalid characters".into()); }
    Ok(sanitized)
}

// ── FFmpeg path helpers ───────────────────────────────────────────────────────

pub(crate) fn ffmpeg_bin_name() -> &'static str {
    if cfg!(target_os = "windows") { "ffmpeg.exe" } else { "ffmpeg" }
}

pub(crate) fn ffprobe_bin_name() -> &'static str {
    if cfg!(target_os = "windows") { "ffprobe.exe" } else { "ffprobe" }
}

// ── SGMCA header stripping ────────────────────────────────────────────────────

pub(crate) fn strip_sgmca_header(src: &Path) -> Result<(PathBuf, bool), String> {
    const MAGIC: &[u8] = b"OggS";
    const SCAN: usize = 8192;

    let mut file = fs::File::open(src).map_err(|e| e.to_string())?;
    let file_size = fs::metadata(src).map(|m| m.len() as usize).unwrap_or(0);
    if file_size == 0 {
        return Err("SGMCA file is empty".into());
    }
    let read_size = SCAN.min(file_size);
    let mut buf = vec![0u8; read_size];
    let bytes_read = file.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(bytes_read);

    let offset = buf.windows(4).position(|w| w == MAGIC).unwrap_or(0);
    if offset == 0 {
        return Ok((src.to_path_buf(), false));
    }

    // Security note: UUID-based temp filenames are unpredictable, which is sufficient
    // for a single-user desktop app. The system temp dir inherits OS-level permissions
    // (typically user-only on macOS/Windows). For multi-user or server contexts, consider
    // creating a private subdirectory with restrictive permissions (0o700).
    let tmp = std::env::temp_dir().join(format!("depoaudio_{}.ogg", Uuid::new_v4().to_string().replace('-', "")));
    file.seek(SeekFrom::Start(offset as u64)).map_err(|e| e.to_string())?;
    let mut out = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut chunk = vec![0u8; 65536];
    loop {
        let n = file.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 { break; }
        out.write_all(&chunk[..n]).map_err(|e| e.to_string())?;
    }
    Ok((tmp, true))
}

// ── Output format helpers ─────────────────────────────────────────────────────

/// Clamp a requested MP3 bitrate to a supported value (kbps). Anything outside
/// the offered set falls back to 192, the long-standing default.
pub(crate) fn mp3_bitrate_kbps(requested: u32) -> u32 {
    match requested {
        128 | 192 | 320 => requested,
        _ => 192,
    }
}

pub(crate) fn output_args(format: &str, rate: &str, mp3_bitrate: u32) -> Vec<String> {
    match format {
        "mp3"  => {
            let br = format!("{}k", mp3_bitrate_kbps(mp3_bitrate));
            vec!["-acodec".into(), "libmp3lame".into(), "-b:a".into(), br, "-ar".into(), rate.into()]
        }
        "flac" => vec!["-c:a".into(), "flac".into(), "-ar".into(), rate.into()],
        "opus" => vec!["-c:a".into(), "libopus".into(), "-b:a".into(), "64k".into(), "-vbr".into(), "on".into(), "-ar".into(), "48000".into()],
        "m4a"  => vec!["-c:a".into(), "aac".into(), "-b:a".into(), "128k".into(), "-ar".into(), rate.into()],
        _      => vec!["-acodec".into(), "pcm_s16le".into(), "-ar".into(), rate.into()],
    }
}

pub(crate) fn output_ext(format: &str) -> &'static str {
    match format {
        "mp3"  => ".mp3",
        "flac" => ".flac",
        "opus" => ".opus",
        "m4a"  => ".m4a",
        _      => ".wav",
    }
}

pub(crate) fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() { return path.to_path_buf(); }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let ext = path.extension().and_then(|e| e.to_str()).map(|e| format!(".{}", e)).unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new("."));
    let mut n = 1;
    loop {
        let candidate = parent.join(format!("{}_{}{}", stem, n, ext));
        if !candidate.exists() { return candidate; }
        n += 1;
    }
}

pub(crate) fn safe_label(s: &str) -> String {
    s.chars().map(|c| if "<>:\"/\\|?* ".contains(c) { '_' } else { c }).collect::<String>().trim().to_string()
}

pub(crate) fn basename(path: &str) -> String {
    Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path).to_string()
}

/// Resample a mono sample buffer with linear interpolation.
pub(crate) fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = (samples.len() as f64 * ratio) as usize;
    (0..out_len)
        .map(|i| {
            let src_pos = i as f64 / ratio;
            let idx = src_pos as usize;
            let frac = src_pos - idx as f64;
            let s0 = samples.get(idx).copied().unwrap_or(0.0);
            let s1 = samples.get(idx + 1).copied().unwrap_or(s0);
            s0 + (s1 - s0) * frac as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_label_sanitizes_special_chars() {
        assert_eq!(safe_label("Speaker 1"), "Speaker_1");
        assert_eq!(safe_label("test<>file"), "test__file");
        assert_eq!(safe_label("a/b\\c"), "a_b_c");
    }

    #[test]
    fn safe_label_trims_edges() {
        // Spaces become underscores, then result is trimmed
        assert_eq!(safe_label("hello"), "hello");
        assert_eq!(safe_label("  hello  "), "__hello__");
    }

    #[test]
    fn infer_case_name_strips_dates() {
        assert_eq!(infer_case_name("Smith_2024-01-15.wav"), "Smith");
        assert_eq!(infer_case_name("Jones_20240115.mp3"), "Jones");
        assert_eq!(infer_case_name("Depo_01-15-2024.wav"), "Depo");
    }

    #[test]
    fn infer_case_name_preserves_words() {
        assert_eq!(infer_case_name("Smith v Jones.wav"), "Smith v Jones");
    }

    #[test]
    fn detect_format_for_path_standard() {
        let wav = detect_format_for_path("test.wav").unwrap();
        assert_eq!(wav.key, "wav");

        let mp3 = detect_format_for_path("test.mp3").unwrap();
        assert_eq!(mp3.key, "mp3");
    }

    #[test]
    fn detect_format_for_path_court() {
        let sgmca = detect_format_for_path("recording.sgmca").unwrap();
        assert_eq!(sgmca.key, "sgmca");

        let trm = detect_format_for_path("session.trm").unwrap();
        assert_eq!(trm.key, "ftr");
    }

    #[test]
    fn detect_format_for_path_unknown() {
        assert!(detect_format_for_path("test.xyz").is_none());
    }

    #[test]
    fn output_ext_matches_format() {
        assert_eq!(output_ext("mp3"), ".mp3");
        assert_eq!(output_ext("flac"), ".flac");
        assert_eq!(output_ext("opus"), ".opus");
        assert_eq!(output_ext("m4a"), ".m4a");
        assert_eq!(output_ext("wav"), ".wav");
        assert_eq!(output_ext("unknown"), ".wav");
    }

    #[test]
    fn mp3_bitrate_clamps_to_supported() {
        assert_eq!(mp3_bitrate_kbps(128), 128);
        assert_eq!(mp3_bitrate_kbps(192), 192);
        assert_eq!(mp3_bitrate_kbps(320), 320);
        // Anything off the menu falls back to 192
        assert_eq!(mp3_bitrate_kbps(0), 192);
        assert_eq!(mp3_bitrate_kbps(256), 192);
        assert_eq!(mp3_bitrate_kbps(9999), 192);
    }

    #[test]
    fn output_args_mp3_uses_selected_bitrate() {
        let args = output_args("mp3", "48000", 320);
        assert!(args.contains(&"libmp3lame".to_string()));
        assert!(args.contains(&"320k".to_string()));

        // Invalid bitrate falls back to 192k
        let fallback = output_args("mp3", "48000", 256);
        assert!(fallback.contains(&"192k".to_string()));

        // Non-MP3 formats ignore the bitrate argument entirely
        let wav = output_args("wav", "48000", 320);
        assert!(wav.contains(&"pcm_s16le".to_string()));
        assert!(!wav.iter().any(|a| a.ends_with('k')));
    }

    #[test]
    fn basename_extracts_filename() {
        assert_eq!(basename("/tmp/audio.wav"), "audio.wav");
        assert_eq!(basename("audio.wav"), "audio.wav");
    }

    #[test]
    fn sanitize_case_name_strips_separators_and_controls() {
        assert_eq!(sanitize_case_name("Smith v Jones"), Ok("Smith v Jones".into()));
        assert_eq!(sanitize_case_name("  padded  "), Ok("padded".into()));
        assert_eq!(sanitize_case_name("a/b\\c:d"), Ok("abcd".into()));
        assert_eq!(sanitize_case_name("tab\there"), Ok("tabhere".into()));
    }

    #[test]
    fn sanitize_case_name_rejects_empty_and_oversized() {
        assert!(sanitize_case_name("").is_err());
        assert!(sanitize_case_name("   ").is_err());
        assert!(sanitize_case_name("///").is_err());
        assert!(sanitize_case_name(&"x".repeat(201)).is_err());
        assert!(sanitize_case_name(&"x".repeat(200)).is_ok());
    }

    #[test]
    fn is_standard_format_checks_correctly() {
        assert!(is_standard_format("wav"));
        assert!(is_standard_format("mp3"));
        assert!(!is_standard_format("sgmca"));
        assert!(!is_standard_format("trm"));
    }
}
