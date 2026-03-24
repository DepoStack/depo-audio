use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use regex::Regex;
use uuid::Uuid;

use crate::types::FormatInfo;

// ── Format registry ───────────────────────────────────────────────────────────

pub(crate) fn get_formats() -> Vec<FormatInfo> {
    vec![
        FormatInfo { key: "sgmca".into(), name: "Stenograph SGMCA".into(), vendor: "Case CATalyst".into(),
            status: "supported".into(), handler: "sgmca".into(), channels: Some("4".into()), note: None },
        FormatInfo { key: "ftr".into(), name: "FTR Recording".into(), vendor: "For The Record".into(),
            status: "experimental".into(), handler: "ftr".into(), channels: Some("4–16".into()),
            note: Some("FTR uses proprietary AAC codec tag 0x4180. Drop all .trm files for a session together.".into()) },
        FormatInfo { key: "aes".into(), name: "Eclipse AudioSync".into(), vendor: "Eclipse CAT".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some("AES-128 encrypted. Open in Eclipse → File → Export Audio → WAV first.".into()) },
        FormatInfo { key: "digitalcat".into(), name: "DigitalCAT Audio".into(), vendor: "Stenovations".into(),
            status: "experimental".into(), handler: "passthrough".into(), channels: None,
            note: Some("No public spec — conversion may fail. Please report results on GitHub.".into()) },
        FormatInfo { key: "bwf".into(), name: "Broadcast WAV".into(), vendor: "CourtSmart / Various".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "generic".into(), name: "WAV · MP3 · FLAC · WMA · M4A · OGG · Opus + more".into(),
            vendor: "Eclipse · ProCAT · StenoCAT · Standard".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
    ]
}

pub(crate) fn detect_format_for_path(path: &str) -> Option<FormatInfo> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let fmts = get_formats();
    match ext.as_str() {
        "sgmca"                                            => fmts.into_iter().find(|f| f.key == "sgmca"),
        "trm" | "ftr"                                      => fmts.into_iter().find(|f| f.key == "ftr"),
        "aes"                                              => fmts.into_iter().find(|f| f.key == "aes"),
        "dm"                                               => fmts.into_iter().find(|f| f.key == "digitalcat"),
        "bwf"                                              => fmts.into_iter().find(|f| f.key == "bwf"),
        "wav"|"mp3"|"flac"|"wma"|"m4a"|"aac"|"ogg"|"opus"|"aif"|"aiff" => fmts.into_iter().find(|f| f.key == "generic"),
        _                                                  => None,
    }
}

// ── Case name detection ───────────────────────────────────────────────────────

pub(crate) fn infer_case_name(filename: &str) -> String {
    let stem = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);

    let date_re = Regex::new(r"[_\-]?\d{4}[_\-]\d{2}[_\-]\d{2}|[_\-]?\d{8}|[_\-]?\d{2}[_\-]\d{2}[_\-]\d{4}").unwrap();
    let cleaned = date_re.replace_all(stem, "");

    let spaced = cleaned.replace('_', " ");
    let words: Vec<&str> = spaced.split_whitespace().collect();
    if words.is_empty() {
        return stem.to_string();
    }
    words.join(" ")
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
    let mut buf = vec![0u8; SCAN.min(fs::metadata(src).map(|m| m.len() as usize).unwrap_or(SCAN))];
    file.read_exact(&mut buf).ok();

    let offset = buf.windows(4).position(|w| w == MAGIC).unwrap_or(0);
    if offset == 0 {
        return Ok((src.to_path_buf(), false));
    }

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

pub(crate) fn output_args(format: &str, rate: &str) -> Vec<String> {
    match format {
        "mp3"  => vec!["-acodec".into(), "libmp3lame".into(), "-b:a".into(), "192k".into(),  "-ar".into(), rate.into()],
        "flac" => vec!["-c:a".into(), "flac".into(), "-ar".into(), rate.into()],
        "opus" => vec!["-c:a".into(), "libopus".into(), "-b:a".into(), "64k".into(), "-vbr".into(), "on".into(), "-ar".into(), "48000".into()],
        _      => vec!["-acodec".into(), "pcm_s16le".into(), "-ar".into(), rate.into()],
    }
}

pub(crate) fn output_ext(format: &str) -> &'static str {
    match format {
        "mp3"  => ".mp3",
        "flac" => ".flac",
        "opus" => ".opus",
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
