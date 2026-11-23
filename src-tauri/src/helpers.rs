use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use std::sync::{Arc, OnceLock};

use regex::Regex;

use crate::types::FormatInfo;

fn date_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"[_\-]?\d{4}[_\-]\d{2}[_\-]\d{2}|[_\-]?\d{8}|[_\-]?\d{2}[_\-]\d{2}[_\-]\d{4}").unwrap()
    })
}

// ── Format registry ───────────────────────────────────────────────────────────

// Closed-format guidance. Each note names the program the format's maker
// documents for exporting it, and says nothing about what DepoAudio could open
// if the file were different (it opens none of these). Sources, checked
// 2026-11-21:
// - DS2: OM System ODMS decrypts (with the password) and converts DS2 files.
//   https://audiosupport.omsystem.com/en/?p=1648 (Encrypt and Decrypt a file
//   on ODMS R7)
// - .arf: Webex Recording Converter, Windows only, "Convert To MP4".
//   https://help.webex.com/en-us/article/p62zmq/Convert-an-ARF-recording-to-MP4-format
// - .wrf: Webex WRF2WMV Converter, Windows, writes WMV (which DepoAudio opens).
//   https://help.webex.com/en-us/article/WBX000026388/WRF2WMV-converter
// - .msv/.dvf: Sony Digital Voice Editor; Sony ended its downloads on
//   2017-03-31. https://sony.jp/products/overseas/contents/support/info/info_dve3_20170403.html
// - .dcr without the Liberty / BIS Digital signature: not a layout DepoAudio
//   reads; the recorder software is named only as an example.
const DCR_NOTE: &str = "DCR recordings use a closed format. Export to WAV from the software that recorded it (for example Liberty Court Recorder or BIS Digital), then add the WAV.";
const DCR_READER_NOTE: &str = "Applies marked redactions; password-protected parts are silent; tested with synthetic files only. The layout comes from interoperability notes and has not been checked against a real recording.";
const DCR_IMAGE_NOTE: &str = "This .dcr starts like a camera raw image (TIFF), not like court audio. Check that you picked the right file. If it is a court recording, export it to WAV from the software that recorded it.";
const DS2_NOTE: &str = "DS2 (DSS Pro) dictation files cannot be opened here. Export to WAV with OM System / Olympus dictation software (for example ODMS), then add the WAV. An encrypted DS2 also needs its password in that software.";
const WEBEX_NOTE: &str = "Webex network recordings (.arf, .wrf) cannot be opened here. Convert an .arf to MP4 with the Webex Recording Converter, or a .wrf to WMV with the Webex WRF2WMV Converter (both Windows only), then add the converted file.";
const DM_NOTE: &str = "Reads the audio inside; tested with synthetic files only. Recorded-over blocks are skipped and the audio is kept in file order.";
const DM_UNRECOGNIZED_NOTE: &str = "This .dm file's header isn't one DepoAudio recognizes. DepoAudio reads .dm audio files written by DigitalCAT / SoundSync; check that you picked the right file.";
const SONY_VOICE_NOTE: &str = "Sony IC recorder files (.msv, .dvf) cannot be opened here. Convert to WAV with Sony Digital Voice Editor, then add the WAV. Sony no longer offers it for download, so an existing copy is needed.";

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
        FormatInfo { key: "video".into(), name: "Video Audio Track".into(), vendor: "MP4 / MOV / MKV / AVI / WebM / MPG / VOB / TS / WMV".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None,
            note: Some("The audio track is extracted and converted; video is discarded.".into()) },
        // Court reporting formats — require conversion
        FormatInfo { key: "sgmca".into(), name: "Stenograph SGMCA".into(), vendor: "Case CATalyst".into(),
            status: "supported".into(), handler: "sgmca".into(), channels: Some("1–16".into()),
            note: Some("Ogg Opus with one mono stream per channel (mapping family 255).".into()) },
        FormatInfo { key: "ftr".into(), name: "FTR Recording".into(), vendor: "For The Record".into(),
            status: "experimental".into(), handler: "ftr".into(), channels: Some("4–16".into()),
            note: Some("FTR interleaves one AAC stream per microphone (or per pair of microphones). DepoAudio reads a file with FFmpeg's native FTR decoder when its layout allows, and otherwise splits the streams itself first; the export record says which. This reading is based on interoperability notes and has not yet been checked against a recording made by FTR. Add all .trm, .ftr or .tll files of a session together.".into()) },
        FormatInfo { key: "bwf".into(), name: "Broadcast WAV".into(), vendor: "CourtSmart / Various".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        // The audio is read out of the .dm into a temporary WAV (dm.rs).
        FormatInfo { key: "digitalcat".into(), name: "DigitalCAT Audio".into(), vendor: "Stenovations · DigitalCAT / SoundSync".into(),
            status: "experimental".into(), handler: "digitalcat".into(), channels: None,
            note: Some(DM_NOTE.into()) },
        // A .dm whose first bytes are not the SoundSync audio header.
        FormatInfo { key: "digitalcat-unrecognized".into(), name: "DigitalCAT Audio (unrecognized)".into(), vendor: "Stenovations".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some(DM_UNRECOGNIZED_NOTE.into()) },
        FormatInfo { key: "aes".into(), name: "Eclipse AudioSync".into(), vendor: "Eclipse CAT".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some("AES-128 encrypted. Open in Eclipse → File → Export Audio → WAV first.".into()) },
        // A .dcr with the HGCRLCRS signature (crate::dcr) is demuxed into a
        // temporary WAV feed. Experimental: synthetic files only.
        FormatInfo { key: "dcr".into(), name: "Liberty / BIS Digital DCR".into(), vendor: "Liberty / BIS Digital".into(),
            status: "experimental".into(), handler: "dcr".into(), channels: Some("1–16".into()),
            note: Some(DCR_READER_NOTE.into()) },
        // Any other .dcr: neutral export guidance.
        FormatInfo { key: "dcr-unknown".into(), name: "Digital Court Recording (DCR)".into(), vendor: "Unknown recorder".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some(DCR_NOTE.into()) },
        // A Kodak camera raw image shares the .dcr extension (TIFF-based).
        FormatInfo { key: "dcr-image".into(), name: "Camera Raw Image (DCR)".into(), vendor: "Kodak camera raw".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some(DCR_IMAGE_NOTE.into()) },
        // Recognized so the user gets guidance instead of "Unrecognised file
        // format". DepoAudio never attempts to decode or decrypt these.
        FormatInfo { key: "ds2".into(), name: "DSS Pro Dictation (DS2)".into(), vendor: "OM System / Olympus".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some(DS2_NOTE.into()) },
        FormatInfo { key: "webex".into(), name: "Webex Recording (ARF / WRF)".into(), vendor: "Cisco Webex".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some(WEBEX_NOTE.into()) },
        FormatInfo { key: "sony-voice".into(), name: "Sony IC Recorder (MSV / DVF)".into(), vendor: "Sony".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some(SONY_VOICE_NOTE.into()) },
    ]
}

/// Check if a file extension is a standard audio format (no conversion needed for basic use)
#[allow(dead_code)]
pub(crate) fn is_standard_format(ext: &str) -> bool {
    matches!(
        ext,
        "wav" | "mp3" | "flac" | "m4a" | "aac" | "ogg" | "opus" | "wma" | "aif" | "aiff" | "caf" | "amr"
    )
}

/// Input-decoder arguments that must appear before `-i` when FFmpeg reads an
/// FTR recording as it is.
///
/// FTR chunks contain one modified AAC frame per court channel. FFmpeg's
/// native `ftr` decoder unwraps and repairs those frames before delegating to
/// AAC; the ordinary `aac` decoder cannot read the interleaved file. When the
/// native decoder cannot take a file, DepoAudio does not fall back to plain
/// AAC on it: it splits the channels first (`crate::trm`) and decodes each
/// channel's own ADTS stream.
///
/// SGMCA streams are decoded with libopus, the reference Opus decoder.
/// FFmpeg's built-in Opus decoder has been seen to corrupt a channel of a
/// 16 kHz mapping-family-255 stream (one channel decoding to full-scale
/// noise) that libopus decodes cleanly; every bundled FFmpeg build includes
/// libopus.
pub(crate) fn input_codec_args(path: &Path) -> Vec<String> {
    // Extension only: the FTR handler does not depend on file content, and
    // this runs inside async conversion and probe paths.
    match detect_format_by_extension(&path.to_string_lossy()) {
        Some(f) if f.handler == "ftr" => vec!["-c:a".into(), "ftr".into()],
        Some(f) if f.handler == "sgmca" => vec!["-c:a".into(), "libopus".into()],
        _ => Vec::new(),
    }
}

/// The format the extension names, before any content check. Never touches
/// the file. `.dcr` is the court format and `.ts` is a transport stream here;
/// `detect_format_for_path` refines both from the file's first bytes.
pub(crate) fn detect_format_by_extension(path: &str) -> Option<FormatInfo> {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    let fmts = get_formats();
    let key = match ext.as_str() {
        // Court formats
        "sgmca" => "sgmca",
        "trm" | "ftr" | "tll" => "ftr",
        "aes" => "aes",
        "dm" => "digitalcat",
        "dcr" => "dcr",
        "ds2" => "ds2",
        "arf" | "wrf" => "webex",
        "msv" | "dvf" => "sony-voice",
        "bwf" => "bwf",
        // Standard formats
        "wav" => "wav",
        "mp3" => "mp3",
        "flac" => "flac",
        "m4a" | "aac" => "m4a",
        "ogg" | "opus" => "ogg",
        "wma" => "wma",
        "aif" | "aiff" => "aiff",
        "caf" => "caf",
        "amr" | "3ga" => "amr",
        // Video containers: FFmpeg extracts the audio track
        "mp4" | "mov" | "mkv" | "avi" | "webm" | "m4v" | "3gp" => "video",
        // Legacy legal-video containers (smoke-tested at release by scripts/container-smoke-fixtures.mjs)
        "mpg" | "mpeg" | "vob" | "ts" | "mts" | "m2ts" | "wmv" | "asf" => "video",
        _ => return None,
    };
    fmts.into_iter().find(|f| f.key == key)
}

/// The format of a file: the extension decides, then cheap content
/// checks. A `.dcr` is a Liberty / BIS Digital recording when it starts with
/// their signature, a camera raw image when it starts like a TIFF, and an
/// unknown recording otherwise (an unreadable one keeps the reader); a `.dm`
/// that does not start with the `.dm` audio header is named as such; and a
/// readable `.ts` with no transport-stream sync pattern (most often a
/// TypeScript source file) is left unrecognised. The checks read only a
/// regular file; a FIFO, device or unreadable path keeps the extension's
/// answer, so detection never waits on a pipe. Blocking file I/O: async
/// callers use `detect_format_blocking`.
pub(crate) fn detect_format_for_path(path: &str) -> Option<FormatInfo> {
    let format = detect_format_by_extension(path)?;
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "dcr" => {
            let key = match dcr_content(Path::new(path)) {
                DcrContent::Liberty | DcrContent::Unreadable => return Some(format),
                DcrContent::Tiff => "dcr-image",
                DcrContent::Other => "dcr-unknown",
            };
            get_formats().into_iter().find(|f| f.key == key)
        }
        "dm" if !could_be_dm(Path::new(path)) => get_formats().into_iter().find(|f| f.key == "digitalcat-unrecognized"),
        // .ts is also a TypeScript source extension.
        "ts" if !could_be_transport_stream(Path::new(path)) => None,
        _ => Some(format),
    }
}

/// `detect_format_for_path` on a blocking thread, so a slow network share
/// never stalls the async runtime or the window.
pub(crate) async fn detect_format_blocking(path: String) -> Option<FormatInfo> {
    tauri::async_runtime::spawn_blocking(move || detect_format_for_path(&path))
        .await
        .ok()
        .flatten()
}

/// Read up to `N` leading bytes of a regular file. Detection stays
/// extension-first; this is only used for cheap content checks, so a file
/// that is not a regular file, or cannot be read, returns `None` and the
/// extension decides. The `is_file` check comes before `open`, because
/// opening a FIFO for reading blocks until something writes to it.
fn leading_bytes<const N: usize>(path: &Path) -> Option<(Box<[u8; N]>, usize)> {
    if !fs::metadata(path).ok()?.is_file() {
        return None;
    }
    let mut file = fs::File::open(path).ok()?;
    let mut buffer = Box::new([0u8; N]);
    let mut filled = 0;
    while filled < N {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(_) => return None,
        }
    }
    Some((buffer, filled))
}

/// What a `.dcr` file starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DcrContent {
    /// The `HGCRLCRS` signature (crate::dcr).
    Liberty,
    /// A TIFF header (`II*\0` or `MM\0*`): a Kodak DCR camera raw image.
    Tiff,
    Other,
    /// Not a regular file, or not readable: the extension decides.
    Unreadable,
}

fn dcr_content(path: &Path) -> DcrContent {
    match leading_bytes::<8>(path) {
        None => DcrContent::Unreadable,
        Some((bytes, 8)) if &*bytes == crate::dcr::DCR_MAGIC => DcrContent::Liberty,
        Some((bytes, filled)) if filled >= 4 && (bytes[..4] == *b"II*\0" || bytes[..4] == *b"MM\0*") => {
            DcrContent::Tiff
        }
        Some(_) => DcrContent::Other,
    }
}

/// True unless the file is readable and does not start with the `.dm` audio
/// header (header size 8 and a plausible WAVEFORMATEX). Unreadable files
/// pass, so the extension decides and the conversion reports the problem.
fn could_be_dm(path: &Path) -> bool {
    match leading_bytes::<64>(path) {
        Some((bytes, filled)) => crate::dm::looks_like_dm(&bytes[..filled]),
        None => true,
    }
}

/// How many bytes of a `.ts` file the transport-stream check reads.
const TS_SCAN_BYTES: usize = 16 * 1024;
/// Sync bytes in a row, at the packet stride, that count as a transport
/// stream when the file does not start on a packet boundary.
const TS_RESYNC_PACKETS: usize = 5;

/// MPEG-TS packets start with the sync byte 0x47 every 188 bytes; M2TS/MTS
/// packets carry a 4-byte timestamp first, so the sync byte is at 4 and every
/// 192 bytes. A file that starts on a packet boundary needs the sync byte at
/// its first (and, if long enough, second) packet. A capture that was cut
/// mid-packet, or has a short leading header, passes when the first 16 KiB
/// hold `TS_RESYNC_PACKETS` sync bytes in a row at either stride from any
/// offset, which is how FFmpeg's own MPEG-TS probe resyncs. An unreadable
/// file passes, so the extension decides.
fn could_be_transport_stream(path: &Path) -> bool {
    const SYNC: u8 = 0x47;
    let Some((bytes, filled)) = leading_bytes::<TS_SCAN_BYTES>(path) else {
        return true;
    };
    let bytes = &bytes[..filled];
    let aligned = |first: usize, stride: usize| {
        bytes.len() > first && bytes[first] == SYNC && (bytes.len() <= first + stride || bytes[first + stride] == SYNC)
    };
    if aligned(0, 188) || aligned(4, 192) {
        return true;
    }
    [188usize, 192].iter().any(|&stride| {
        let span = stride * (TS_RESYNC_PACKETS - 1);
        (0..stride.min(bytes.len().saturating_sub(span)))
            .any(|start| (0..TS_RESYNC_PACKETS).all(|packet| bytes[start + packet * stride] == SYNC))
    })
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
    if trimmed.is_empty() {
        return Err("Case name cannot be empty".into());
    }
    if trimmed.len() > 200 {
        return Err("Case name is too long (max 200 characters)".into());
    }
    let sanitized: String = trimmed
        .chars()
        .filter(|c| !c.is_control() && *c != '/' && *c != '\\' && *c != ':')
        .collect();
    if sanitized.is_empty() {
        return Err("Case name contains only invalid characters".into());
    }
    Ok(sanitized)
}

// ── FFmpeg path helpers ───────────────────────────────────────────────────────

pub(crate) fn ffmpeg_bin_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

pub(crate) fn ffprobe_bin_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "ffprobe.exe"
    } else {
        "ffprobe"
    }
}

/// Global/input options for sidecars that must only read a user-selected local
/// file. Keep this before codec hints and `-i`; callers may still use stdout as
/// an output because the protocol whitelist is scoped to the following input.
pub(crate) fn safe_ffmpeg_input_prelude() -> Vec<String> {
    vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-protocol_whitelist".into(),
        "file".into(),
    ]
}

// ── Audio stream selection ────────────────────────────────────────────────────

/// Zero-based index, among the input's audio streams, of the one stream that
/// every probe, Scan pass, waveform, auto-level pass and conversion reads.
/// Without an explicit `-map`, FFmpeg's automatic stream selection picks the
/// "best" audio stream (most channels), which in a file with several audio
/// tracks can differ from the `a:0` the probes and the export record describe.
pub(crate) const SOURCE_AUDIO_STREAM: u32 = 0;

/// ffprobe `-select_streams` value for the converted stream (`a:0`).
pub(crate) fn source_audio_stream_specifier() -> String {
    format!("a:{SOURCE_AUDIO_STREAM}")
}

/// FFmpeg output option that maps exactly the converted stream (`-map 0:a:0`).
/// Place it after `-i` and before the output it applies to.
pub(crate) fn source_audio_map_args() -> Vec<String> {
    vec!["-map".into(), format!("0:{}", source_audio_stream_specifier())]
}

/// Filtergraph input label for the converted stream (`[0:a:0]`).
pub(crate) fn source_audio_filter_input() -> String {
    format!("[0:{}]", source_audio_stream_specifier())
}

// ── SGMCA header stripping ────────────────────────────────────────────────────

pub(crate) const AUDIO_PREPARATION_CANCELLED_MESSAGE: &str = "Audio preparation cancelled";

fn strip_sgmca_header_with_cancel(
    src: &Path,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<(PathBuf, Option<crate::safety::TempFile>), String> {
    let is_cancelled = || cancelled.is_some_and(|check| check());
    if is_cancelled() {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }

    let mut file = fs::File::open(src).map_err(|e| e.to_string())?;
    let file_size = fs::metadata(src).map_err(|e| e.to_string())?.len();
    if file_size == 0 {
        return Err("SGMCA file is empty".into());
    }
    // The stream starts at the first CRC-correct beginning-of-stream page
    // that carries an Opus identification header, so stray "OggS" bytes in
    // a prefix are skipped and a damaged or non-Opus file is named as such
    // before FFmpeg sees it. Plain SGMCA files start at byte 0 and are used
    // in place.
    let offset = crate::sgmca::locate_stream(&mut file)?;
    if is_cancelled() {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    // FFmpeg plays a file with several sessions (one Ogg stream each) as one
    // stream with the gaps between them closed, which moves every later
    // session to the wrong time. Such a file is refused, by name of each
    // session, until sessions can be handled one at a time.
    crate::sgmca::ensure_single_session(&mut file, offset, cancelled).map_err(|error| {
        if is_cancelled() {
            AUDIO_PREPARATION_CANCELLED_MESSAGE.to_string()
        } else {
            error
        }
    })?;
    if is_cancelled() {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    if offset == 0 {
        return Ok((src.to_path_buf(), None));
    }

    // The copy goes in DepoAudio's private work folder (workdir.rs). It
    // keeps the .sgmca extension so every later step treats it as SGMCA
    // (decoder choice included); FFmpeg finds the Ogg stream by content.
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let (guard, mut out) = crate::workdir::new_temp_file("depoaudio_", "sgmca")?;
    // On any error or cancellation the guard drops and removes the copy.
    let mut chunk = vec![0u8; 65536];
    loop {
        if is_cancelled() {
            return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
        }
        let n = file.read(&mut chunk).map_err(|error| error.to_string())?;
        if n == 0 {
            break;
        }
        out.write_all(&chunk[..n]).map_err(|error| error.to_string())?;
    }
    if is_cancelled() {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    // Read back by this run only: no sync_all.
    Ok((guard.path.clone(), Some(guard)))
}

#[cfg(test)]
pub(crate) fn strip_sgmca_header(src: &Path) -> Result<(PathBuf, Option<crate::safety::TempFile>), String> {
    strip_sgmca_header_with_cancel(src, None)
}

/// What the caller knows about the FFmpeg that will read the feed.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FeedOptions {
    /// The FFmpeg build has its native `ftr` decoder.
    pub native_ftr: bool,
    /// An earlier native `ftr` attempt on this file failed.
    pub native_failed: bool,
}

const MERGED_SOURCE_LABEL: &str = "[da_src]";
const MERGED_OUTPUT_LABEL: &str = "[da_out]";

/// The audio FFmpeg reads for one selected file: usually the file itself; an
/// SGMCA's Ogg stream in a temporary copy; a `.dm`'s or a Liberty / BIS DCR's audio read into a
/// temporary WAV; or, for an FTR recording that
/// FFmpeg's native decoder cannot take as it is, one temporary mono ADTS file
/// per channel, joined back in channel order with `amerge`. Temporary files
/// live exactly as long as the feed, in DepoAudio's private work folder
/// (workdir.rs).
#[derive(Debug)]
pub(crate) struct AudioFeed {
    inputs: Vec<PathBuf>,
    /// Options placed before every `-i` (a decoder or demuxer choice).
    input_codec: Vec<String>,
    trm: Option<crate::trm::TrmRoute>,
    /// Channels in each input: 1, or 2 for a stereo-stream FTR recording
    /// split into one stereo file per pair of microphones.
    channels_per_input: u32,
    /// An FTR recording DepoAudio could not read, handed to FFmpeg's native
    /// decoder as it is: why the inspection failed.
    trm_inspection_error: Option<String>,
    /// For a `.dm`: its blocks and time stamps, for the export record.
    dm_timeline: Option<crate::dm::DmTimeline>,
    /// What the DCR reader found, for a Liberty / BIS Digital `.dcr`.
    dcr: Option<crate::dcr::DcrInfo>,
    /// For a DCR whose codec FFmpeg decodes one channel per stream: the
    /// FFmpeg run that joins the channels into `inputs[0]`
    /// (`finish_prepared_feed`).
    merge: Option<crate::dcr::MergeStep>,
    _guards: Vec<crate::safety::TempFile>,
    /// The folder holding the channel files; dropped after them.
    _channel_dir: Option<crate::workdir::TempDir>,
}

impl AudioFeed {
    /// The file itself, with the decoder its extension calls for (libopus
    /// for SGMCA, see [`input_codec_args`]) and otherwise FFmpeg's own
    /// choice. An FTR file read this way (MP2, MP3 or WMA audio) gets no
    /// forced decoder: the native `ftr` decoder is only for FTR AAC.
    pub(crate) fn plain(path: PathBuf) -> Self {
        let input_codec = if is_trm_path(&path) {
            Vec::new()
        } else {
            input_codec_args(&path)
        };
        Self {
            inputs: vec![path],
            input_codec,
            trm: None,
            channels_per_input: 1,
            trm_inspection_error: None,
            dm_timeline: None,
            dcr: None,
            merge: None,
            _guards: Vec::new(),
            _channel_dir: None,
        }
    }

    /// The first input: the file ffprobe reads when nothing is known.
    pub(crate) fn path(&self) -> &Path {
        &self.inputs[0]
    }

    /// The FTR route taken, for the export record.
    pub(crate) fn trm(&self) -> Option<&crate::trm::TrmRoute> {
        self.trm.as_ref()
    }

    /// For a `.dm`: its blocks and time stamps, for the export record.
    pub(crate) fn dm_timeline(&self) -> Option<&crate::dm::DmTimeline> {
        self.dm_timeline.as_ref()
    }

    /// For a Liberty / BIS Digital `.dcr`: what the reader found.
    pub(crate) fn dcr(&self) -> Option<&crate::dcr::DcrInfo> {
        self.dcr.as_ref()
    }

    /// The DCR channel merge still to run before the feed can be read, if
    /// any, taken out of the feed ([`finish_prepared_feed`] runs it).
    pub(crate) fn take_merge(&mut self) -> Option<crate::dcr::MergeStep> {
        self.merge.take()
    }

    /// Why DepoAudio could not read this FTR recording itself, when it was
    /// handed to FFmpeg's native decoder as it is instead.
    pub(crate) fn trm_inspection_error(&self) -> Option<&str> {
        self.trm_inspection_error.as_deref()
    }

    /// Several inputs that `amerge` joins into the converted stream.
    pub(crate) fn is_merged(&self) -> bool {
        self.inputs.len() > 1
    }

    /// Every input in order, each preceded by `per_input` and the feed's own
    /// input options: `[per_input] [-c:a ftr | -f aac] -i <input>`.
    pub(crate) fn input_args(&self, per_input: &[String]) -> Vec<String> {
        let mut args = Vec::new();
        for input in &self.inputs {
            args.extend(per_input.iter().cloned());
            args.extend(self.input_codec.iter().cloned());
            args.extend(["-i".into(), input.to_string_lossy().to_string()]);
        }
        args
    }

    /// The arguments for the input that holds one channel, when the feed
    /// has one input per channel or per pair (so a per-channel pass decodes
    /// only that input), and the channel's index inside that input when the
    /// input holds more than one.
    pub(crate) fn channel_input_args(&self, channel: u32, per_input: &[String]) -> Option<(Vec<String>, Option<u32>)> {
        if !self.is_merged() {
            return None;
        }
        let per = self.channels_per_input.max(1);
        let input = self.inputs.get((channel / per) as usize)?;
        let mut args: Vec<String> = per_input.to_vec();
        args.extend(self.input_codec.iter().cloned());
        args.extend(["-i".into(), input.to_string_lossy().to_string()]);
        Some((args, (per > 1).then_some(channel % per)))
    }

    /// `[0:a:0][1:a:0]…amerge=inputs=N`: the joined stream holds input 0's
    /// channels, then input 1's, and so on (for stereo inputs, channels 2k
    /// and 2k + 1 are input k's).
    fn merge_filter(&self) -> String {
        let labels: String = (0..self.inputs.len())
            .map(|index| format!("[{index}:{}]", source_audio_stream_specifier()))
            .collect();
        format!("{labels}amerge=inputs={}", self.inputs.len())
    }

    /// The filtergraph prefix (empty, or the `amerge` step ending in `;`)
    /// and the label of the converted stream inside a `-filter_complex`.
    pub(crate) fn filter_source(&self) -> (String, String) {
        if self.is_merged() {
            (
                format!("{}{MERGED_SOURCE_LABEL};", self.merge_filter()),
                MERGED_SOURCE_LABEL.into(),
            )
        } else {
            (String::new(), source_audio_filter_input())
        }
    }

    /// How an output reads the converted stream with an optional filter
    /// chain: `-map 0:a:0 [-af <chain>]` for one input, or, for joined
    /// inputs, `-filter_complex <amerge>[,<chain>][da_out] -map [da_out]`
    /// (FFmpeg does not allow `-af` on a filtergraph's output).
    pub(crate) fn source_output_args(&self, chain: Option<&str>) -> Vec<String> {
        let (option, graph, map) = self.source_filtering(chain);
        let mut args = Vec::new();
        if let (Some(option), Some(graph)) = (option, graph) {
            if option == "-filter_complex" {
                args.extend([option.to_string(), graph]);
                args.extend(map);
            } else {
                args.extend(map);
                args.extend([option.to_string(), graph]);
            }
        } else {
            args.extend(map);
        }
        args
    }

    /// The parts of [`Self::source_output_args`]: filter option, filter
    /// graph or chain, and the `-map` arguments.
    pub(crate) fn source_filtering(&self, chain: Option<&str>) -> (Option<&'static str>, Option<String>, Vec<String>) {
        if self.is_merged() {
            let graph = match chain {
                Some(chain) => format!("{},{chain}{MERGED_OUTPUT_LABEL}", self.merge_filter()),
                None => format!("{}{MERGED_OUTPUT_LABEL}", self.merge_filter()),
            };
            (
                Some("-filter_complex"),
                Some(graph),
                vec!["-map".into(), MERGED_OUTPUT_LABEL.into()],
            )
        } else {
            match chain {
                Some(chain) => (Some("-af"), Some(chain.to_string()), source_audio_map_args()),
                None => (None, None, source_audio_map_args()),
            }
        }
    }

    /// The FTR route when DepoAudio read the recording's audio chunks
    /// itself; None for other files and for an FTR recording whose audio
    /// (MP2, MP3, WMA) FFmpeg reads as it is.
    fn trm_read(&self) -> Option<&crate::trm::TrmRoute> {
        self.trm
            .as_ref()
            .filter(|route| route.decode_path != crate::trm::DecodePath::Plain)
    }

    /// DepoAudio read this FTR recording's frames, so its duration and
    /// channel count come from that read, not from ffprobe.
    pub(crate) fn trm_frames_read(&self) -> bool {
        self.trm_read().is_some()
    }

    /// Channel count read from the recording itself (FTR), if any.
    pub(crate) fn known_channels(&self) -> Option<u32> {
        self.trm_read().map(|route| u32::from(route.info.channels))
    }

    /// Duration from the frames actually read (FTR): never from a header
    /// length an unclosed recording leaves at its placeholder.
    pub(crate) fn known_duration(&self) -> Option<f64> {
        self.trm_read().and_then(|route| route.info.duration_secs)
    }

    /// FFmpeg reads this FTR recording with its native decoder.
    pub(crate) fn used_native_ftr(&self) -> bool {
        self.trm
            .as_ref()
            .is_some_and(|route| route.decode_path == crate::trm::DecodePath::NativeFtr)
    }

    pub(crate) fn known_sample_rate(&self) -> Option<u32> {
        self.trm_read().and_then(|route| route.info.sample_rate)
    }
}

/// The name prefix of each conversion's own channel-file folder inside
/// this run's private work folder: `depoaudio_trm_run_<32 hex>`.
const CHANNEL_DIR_PREFIX: &str = "depoaudio_trm_run_";

/// The selected file is an FTR recording (`.trm`, `.ftr`, `.tll`).
pub(crate) fn is_trm_path(path: &Path) -> bool {
    detect_format_by_extension(&path.to_string_lossy()).is_some_and(|format| format.handler == "ftr")
}

/// An FTR recording handed to FFmpeg's native decoder as it is, without
/// DepoAudio's layout reading; `reason` says why, for the export record.
fn native_as_is(src: &Path, reason: String) -> AudioFeed {
    AudioFeed {
        inputs: vec![src.to_path_buf()],
        input_codec: input_codec_args(src),
        trm: None,
        channels_per_input: 1,
        trm_inspection_error: Some(reason),
        dm_timeline: None,
        dcr: None,
        merge: None,
        _guards: Vec::new(),
        _channel_dir: None,
    }
}

fn prepare_trm_feed(
    src: &Path,
    options: FeedOptions,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<AudioFeed, String> {
    let info = match crate::trm::inspect_trm_with_cancel(src, cancelled) {
        Ok(info) => info,
        Err(error) if cancelled.is_some_and(|check| check()) || error == crate::trm::TRM_CANCELLED_MESSAGE => {
            return Err(error)
        }
        // FTR's own splitter refuses this layout: so does DepoAudio, by
        // every route.
        Err(error) if crate::trm::is_refusal(&error) => return Err(error),
        // DepoAudio's reading of the layout is unconfirmed, so a file it
        // cannot read goes to FFmpeg's native decoder as it is, as before;
        // the export record gives the reason.
        Err(error) if options.native_ftr && !options.native_failed => return Ok(native_as_is(src, error)),
        Err(error) => return Err(error),
    };
    let (decode_path, reasons) = crate::trm::choose_route(&info, options);
    // DepoAudio read the layout but found no complete round to split (its
    // stream-marker rule, from notes never checked against a real FTR
    // recording, may not fit this recorder): the native decoder, which does
    // not use that rule, reads the file as it is, as for an unreadable one.
    if decode_path == crate::trm::DecodePath::Deinterleaved
        && info.frames_per_channel == 0
        && options.native_ftr
        && !options.native_failed
    {
        return Ok(native_as_is(src, crate::trm::NO_COMPLETE_ROUND_MESSAGE.to_string()));
    }
    match decode_path {
        crate::trm::DecodePath::NativeFtr => Ok(AudioFeed {
            inputs: vec![src.to_path_buf()],
            input_codec: input_codec_args(src),
            trm: Some(crate::trm::TrmRoute::new(info, decode_path, reasons)),
            channels_per_input: 1,
            trm_inspection_error: None,
            dm_timeline: None,
            dcr: None,
            merge: None,
            _guards: Vec::new(),
            _channel_dir: None,
        }),
        // MP2, MP3 or WMA in an FTR file: FFmpeg picks the decoder.
        crate::trm::DecodePath::Plain => Ok(AudioFeed {
            trm: Some(crate::trm::TrmRoute::new(info, decode_path, reasons)),
            ..AudioFeed::plain(src.to_path_buf())
        }),
        crate::trm::DecodePath::Deinterleaved => {
            let channel_dir = crate::workdir::new_temp_dir(CHANNEL_DIR_PREFIX)?;
            let (written, files) =
                crate::trm::deinterleave_trm_with_cancel(src, channel_dir.path(), Some(&info), cancelled)?;
            let guards = files.iter().cloned().map(crate::safety::TempFile::new).collect();
            let channels_per_input = u32::from(written.channels_per_stream.max(1));
            Ok(AudioFeed {
                inputs: files,
                input_codec: vec!["-f".into(), "aac".into()],
                trm: Some(crate::trm::TrmRoute::new(written, decode_path, reasons)),
                channels_per_input,
                trm_inspection_error: None,
                dm_timeline: None,
                dcr: None,
                merge: None,
                _guards: guards,
                _channel_dir: Some(channel_dir),
            })
        }
    }
}

// ── DigitalCAT .dm demux ──────────────────────────────────────────────────────

/// Read the audio of a `.dm` into a new temporary RIFF WAVE file (see
/// `dm.rs`). The original is only read. The copy is removed when the
/// returned guard drops, and on any error or cancellation before that.
fn demux_dm_with_cancel(
    src: &Path,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<(crate::safety::TempFile, crate::dm::DmTimeline), String> {
    let is_cancelled = || cancelled.is_some_and(|check| check());
    let dm = crate::dm::parse_dm(src, &is_cancelled)?;
    let mut timeline = crate::dm::build_timeline(&dm);
    // Counts only; marker text is never kept (see bmf.rs).
    timeline.marker_file = crate::bmf::summary_beside(src, &dm);
    if crate::dm::audio_totals(&dm).0 == 0 {
        return Err("This .dm file has no audio left to convert: every block was recorded over or empty.".into());
    }
    // In the private work folder, like the SGMCA copy above.
    let (guard, file) = crate::workdir::new_temp_file("depoaudio_", "wav")?;
    let mut writer = std::io::BufWriter::with_capacity(256 * 1024, file);
    crate::dm::write_wav(src, &dm, &mut writer, &is_cancelled)?;
    // Read back by this run only: flushed, not sync_all'ed.
    writer
        .into_inner()
        .map_err(|error| format!("Could not write the temporary audio copy: {error}"))?;
    if is_cancelled() {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    Ok((guard, timeline))
}

pub(crate) fn has_extension(src: &Path, wanted: &str) -> bool {
    src.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
}

/// Prepare the selected file for FFmpeg: an SGMCA loses its vendor prefix,
/// a `.dm` has its audio read into a WAV, a Liberty / BIS DCR is demuxed
/// into a WAV, and an FTR recording is read as it is by FFmpeg's native
/// decoder or split into one temporary file per stream; every probe, scan,
/// waveform and conversion reads the result. Any other file is used as it
/// is. A DCR may still need its channel merge (`finish_prepared_feed`).
pub(crate) fn prepare_audio_feed_with_cancel(
    src: &Path,
    options: FeedOptions,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<AudioFeed, String> {
    if cancelled.is_some_and(|check| check()) {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    if has_extension(src, "dcr") {
        return match dcr_content(src) {
            DcrContent::Liberty | DcrContent::Unreadable => {
                let never = || false;
                let check: &dyn Fn() -> bool = match cancelled {
                    Some(check) => check,
                    None => &never,
                };
                let feed = crate::dcr::prepare_feed(src, check)?;
                let mut prepared = AudioFeed::plain(feed.path);
                prepared._guards.push(feed.guard);
                prepared.merge = feed.merge;
                prepared.dcr = Some(feed.info);
                Ok(prepared)
            }
            DcrContent::Tiff => Err(DCR_IMAGE_NOTE.into()),
            DcrContent::Other => Err(DCR_NOTE.into()),
        };
    }
    if is_trm_path(src) {
        return prepare_trm_feed(src, options, cancelled);
    }
    if has_extension(src, "dm") {
        let (guard, timeline) = demux_dm_with_cancel(src, cancelled)?;
        let mut prepared = AudioFeed::plain(guard.path.clone());
        prepared.dm_timeline = Some(timeline);
        prepared._guards.push(guard);
        return Ok(prepared);
    }
    if !has_extension(src, "sgmca") {
        return Ok(AudioFeed::plain(src.to_path_buf()));
    }

    let (feed, guard) = strip_sgmca_header_with_cancel(src, cancelled)?;
    let mut prepared = AudioFeed::plain(feed);
    prepared._guards.extend(guard);
    Ok(prepared)
}

/// How long the FFmpeg run that joins a DCR's channels may take.
const DCR_MERGE_TIMEOUT_SECS: u64 = 4 * 60 * 60;

/// Run a prepared feed's remaining FFmpeg step, if any, with the bundled
/// sidecar. The DCR's one-channel temporary WAVs are removed afterwards
/// either way; on failure the partial feed is removed too.
pub(crate) async fn finish_prepared_feed(
    app: &tauri::AppHandle,
    mut prepared: AudioFeed,
    cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<AudioFeed, String> {
    let Some(merge) = prepared.take_merge() else {
        return Ok(prepared);
    };
    let crate::dcr::MergeStep { inputs, args } = merge;
    let output = crate::ffmpeg::sidecar_output_cancellable(
        app,
        ffmpeg_bin_name(),
        args,
        DCR_MERGE_TIMEOUT_SECS,
        Some(cancelled),
    )
    .await;
    drop(inputs);
    if cancelled() {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    let codec = prepared.dcr.as_ref().map(|info| info.codec.clone()).unwrap_or_default();
    match output {
        Some(output) if output.success && prepared.path().is_file() => {
            crate::dcr::clear_channel_mask(prepared.path())?;
            Ok(prepared)
        }
        Some(output) => Err(dcr_merge_error(&codec, &output.stderr)),
        None => Err(dcr_merge_error(&codec, b"")),
    }
}

/// The error for a failed DCR channel merge, with FFmpeg's last message.
/// The merge names temporary files in DepoAudio's work folder, under the
/// user's profile (which holds the account name), so a message that mentions a path keeps
/// only its last part after `: ` when that part has no path in it (for
/// example "No space left on device"), and is left out otherwise.
pub(crate) fn dcr_merge_error(codec: &str, stderr: &[u8]) -> String {
    let log = String::from_utf8_lossy(stderr);
    let last = log
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    let has_path = |text: &str| text.contains(['/', '\\']) || text.contains("depoaudio_");
    let shown = if has_path(last) {
        last.rsplit(": ")
            .next()
            .filter(|tail| *tail != last && !has_path(tail))
            .unwrap_or("")
    } else {
        last
    };
    let shown: String = shown.trim().chars().take(300).collect();
    let mut message = format!("FFmpeg could not decode this recording's {codec} audio.");
    if !shown.is_empty() {
        message.push_str(&format!(" FFmpeg said: {shown}"));
    }
    message
}

/// Prepare a feed without blocking the async command executor, and run any
/// remaining FFmpeg step (a DCR's channel merge). The cancellation predicate
/// is checked between every 64 KiB read or copied (65,536 atoms for a DCR)
/// and owns no borrowed command state, so scan/conversion/waveform callers
/// can reuse it. For an FTR recording this asks the bundled FFmpeg (once per
/// process) whether it has the native `ftr` decoder.
pub(crate) async fn prepare_audio_feed_cancellable(
    app: &tauri::AppHandle,
    src: PathBuf,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<AudioFeed, String> {
    let options = crate::ffmpeg::feed_options(app, &src).await;
    let prepared = prepare_audio_feed_with_options(src, options, cancelled.clone()).await?;
    finish_prepared_feed(app, prepared, cancelled.as_ref()).await
}

/// The blocking part of [`prepare_audio_feed_cancellable`], with the feed
/// options already known, on a blocking thread. A DCR's channel merge is not
/// run here: callers that can get one pass the feed to
/// [`finish_prepared_feed`].
pub(crate) async fn prepare_audio_feed_with_options(
    src: PathBuf,
    options: FeedOptions,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<AudioFeed, String> {
    tauri::async_runtime::spawn_blocking(move || {
        prepare_audio_feed_with_cancel(&src, options, Some(cancelled.as_ref()))
    })
    .await
    .map_err(|error| format!("Audio preparation task failed: {error}"))?
}

/// The feed for an FTR recording on which FFmpeg's native decoder already
/// failed: always the split-channel route, with "native-decode-failed" among
/// the reasons the export record gives.
pub(crate) async fn prepare_audio_feed_after_native_failure(
    app: &tauri::AppHandle,
    src: PathBuf,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<AudioFeed, String> {
    let options = FeedOptions {
        native_ftr: true,
        native_failed: true,
    };
    let prepared = prepare_audio_feed_with_options(src, options, cancelled.clone()).await?;
    finish_prepared_feed(app, prepared, cancelled.as_ref()).await
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
    let codec: Vec<String> = match format {
        "mp3" => {
            let br = format!("{}k", mp3_bitrate_kbps(mp3_bitrate));
            vec![
                "-acodec".into(),
                "libmp3lame".into(),
                "-b:a".into(),
                br,
                "-ar".into(),
                rate.into(),
            ]
        }
        "flac" => vec!["-c:a".into(), "flac".into(), "-ar".into(), rate.into()],
        "opus" => vec![
            "-c:a".into(),
            "libopus".into(),
            "-b:a".into(),
            "64k".into(),
            "-vbr".into(),
            "on".into(),
            "-ar".into(),
            "48000".into(),
        ],
        // Stenograph Direct Audio: Ogg Opus with one uncoupled mono stream
        // per channel (mapping family 255) and a 16 kHz input rate tag.
        // The bitrate depends on the channel count, so the conversion plan
        // adds it. `-f ogg` because FFmpeg does not know the extension.
        "sgmca" => vec![
            "-c:a".into(),
            "libopus".into(),
            "-mapping_family".into(),
            "255".into(),
            "-frame_duration".into(),
            "20".into(),
            "-vbr".into(),
            "on".into(),
            "-ar".into(),
            "16000".into(),
            "-f".into(),
            "ogg".into(),
        ],
        "m4a" => vec![
            "-c:a".into(),
            "aac".into(),
            "-b:a".into(),
            "128k".into(),
            "-ar".into(),
            rate.into(),
        ],
        // Plain RIFF. A WAV that may pass 4 GiB also gets `wav_rf64_args`.
        _ => vec!["-acodec".into(), "pcm_s16le".into(), "-ar".into(), rate.into()],
    };
    // Outputs are audio only. Without -vn, FFmpeg maps a video input's picture
    // into an M4A output (the ipod muxer accepts H.264), and the LGPL sidecars
    // have no H.264 encoder, so the conversion would fail or carry video.
    let mut args: Vec<String> = vec!["-vn".into(), "-sn".into()];
    args.extend(codec);
    args
}

/// Estimated WAV outputs at or above this size are written with `-rf64 auto`
/// (3.5 GiB, so the estimate has room below the 4 GiB RIFF size limit).
pub(crate) const RF64_ESTIMATE_BYTES: u64 = 3_758_096_384;

/// Estimated size of a 16-bit PCM WAV output: duration × channels × rate × 2.
/// None when any input is unknown.
pub(crate) fn wav_estimated_bytes(duration_secs: Option<f64>, channels: Option<u32>, rate: Option<u32>) -> Option<u64> {
    let duration = duration_secs.filter(|duration| duration.is_finite() && *duration > 0.0)?;
    let bytes = duration * f64::from(channels?) * f64::from(rate?) * 2.0;
    Some(bytes.ceil() as u64)
}

/// `-rf64 auto` for a WAV output that may pass the 4 GiB RIFF size fields (a
/// 16-channel 48 kHz Keep Original WAV does after about 46 minutes), or whose
/// size cannot be estimated. FFmpeg then writes plain RIFF while the file fits
/// and RF64 once it outgrows them, but it also reserves room for the RF64
/// size chunk with a JUNK chunk before `fmt `. Every other WAV gets no RF64
/// option, so its header stays the plain RIFF layout earlier versions wrote.
pub(crate) fn wav_rf64_args(estimated_bytes: Option<u64>) -> Vec<String> {
    match estimated_bytes {
        Some(bytes) if bytes < RF64_ESTIMATE_BYTES => Vec::new(),
        _ => vec!["-rf64".into(), "auto".into()],
    }
}

pub(crate) fn output_ext(format: &str) -> &'static str {
    match format {
        "mp3" => ".mp3",
        "flac" => ".flac",
        "opus" => ".opus",
        "m4a" => ".m4a",
        "sgmca" => ".sgmca",
        _ => ".wav",
    }
}

/// Atomically reserve a unique output path by creating an empty placeholder.
///
/// Checking `Path::exists` before starting FFmpeg is racy: two jobs can both
/// choose the same filename and `-y` lets the later encoder overwrite the
/// earlier one. `create_new` makes selection and reservation one filesystem
/// operation. Callers must remove the placeholder if the job fails; FFmpeg
/// overwrites it on success.
pub(crate) fn reserve_unique_path(path: &Path) -> Result<PathBuf, String> {
    reserve_unique_path_with(path, |_| Ok(Some(()))).map(|(path, ())| path)
}

/// `reserve_unique_path` plus a companion reservation for each candidate.
/// `companion` runs after the candidate is created; `Ok(None)` means the
/// companion name is taken, so the candidate placeholder is released and the
/// next numbered name is tried. Used to keep an output and its export record
/// (`<output>.depoaudio.json`) on matching, never-overwritten names.
pub(crate) fn reserve_unique_path_with<T>(
    path: &Path,
    mut companion: impl FnMut(&Path) -> Result<Option<T>, String>,
) -> Result<(PathBuf, T), String> {
    const MAX_ATTEMPTS: u32 = 1_000;

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("out");
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e))
        .unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new("."));

    for suffix in 0..MAX_ATTEMPTS {
        let candidate = if suffix == 0 {
            path.to_path_buf()
        } else {
            parent.join(format!("{}_{}{}", stem, suffix, ext))
        };

        match fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => {
                drop(file);
                match companion(&candidate) {
                    Ok(Some(value)) => return Ok((candidate, value)),
                    Ok(None) => {
                        let _ = fs::remove_file(&candidate);
                        continue;
                    }
                    Err(error) => {
                        let _ = fs::remove_file(&candidate);
                        return Err(error);
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not reserve an output file in the selected folder: {error}"
                ));
            }
        }
    }

    Err(format!(
        "Could not reserve a unique output filename after {MAX_ATTEMPTS} attempts"
    ))
}

pub(crate) fn safe_label(s: &str) -> String {
    s.chars()
        .map(|c| if "<>:\"/\\|?* ".contains(c) { '_' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

pub(crate) fn basename(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use std::sync::{Arc, Barrier};
    use std::thread;

    struct ReservationTestDir(PathBuf);

    impl ReservationTestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "depoaudio_reservation_test_{}",
                uuid::Uuid::new_v4().to_string().replace('-', "")
            ));
            fs::create_dir(&path).expect("create reservation test directory");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for ReservationTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

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
        assert_eq!(trm.handler, "ftr");

        let ftr = detect_format_for_path("SESSION.FTR").unwrap();
        assert_eq!(ftr.key, "ftr");
        assert_eq!(ftr.handler, "ftr");

        for name in ["session.tll", "SESSION.TLL"] {
            let tll = detect_format_for_path(name).unwrap();
            assert_eq!((tll.key.as_str(), tll.handler.as_str()), ("ftr", "ftr"), "{name}");
            assert!(is_trm_path(Path::new(name)));
        }
        assert!(!is_trm_path(Path::new("session.trs")));
        assert!(detect_format_for_path("session.trs").is_none());
    }

    #[test]
    fn a_merged_feed_joins_its_inputs_in_channel_order() {
        let feed = AudioFeed {
            inputs: (0..3)
                .map(|channel| PathBuf::from(format!("/tmp/ch{channel}.aac")))
                .collect(),
            input_codec: vec!["-f".into(), "aac".into()],
            trm: None,
            channels_per_input: 1,
            trm_inspection_error: None,
            dm_timeline: None,
            dcr: None,
            merge: None,
            _guards: Vec::new(),
            _channel_dir: None,
        };
        assert!(feed.is_merged());
        assert_eq!(
            feed.input_args(&["-t".into(), "5".into()]),
            [
                "-t",
                "5",
                "-f",
                "aac",
                "-i",
                "/tmp/ch0.aac",
                "-t",
                "5",
                "-f",
                "aac",
                "-i",
                "/tmp/ch1.aac",
                "-t",
                "5",
                "-f",
                "aac",
                "-i",
                "/tmp/ch2.aac"
            ]
        );
        assert_eq!(
            feed.source_output_args(Some("volume=2")),
            [
                "-filter_complex",
                "[0:a:0][1:a:0][2:a:0]amerge=inputs=3,volume=2[da_out]",
                "-map",
                "[da_out]"
            ]
        );
        assert_eq!(
            feed.source_output_args(None),
            [
                "-filter_complex",
                "[0:a:0][1:a:0][2:a:0]amerge=inputs=3[da_out]",
                "-map",
                "[da_out]"
            ]
        );
        assert_eq!(
            feed.filter_source(),
            (
                "[0:a:0][1:a:0][2:a:0]amerge=inputs=3[da_src];".to_string(),
                "[da_src]".to_string()
            )
        );
        let (args, inner) = feed.channel_input_args(2, &[]).unwrap();
        assert_eq!(args, ["-f", "aac", "-i", "/tmp/ch2.aac"]);
        assert_eq!(inner, None);
        assert!(feed.channel_input_args(3, &[]).is_none());

        // A stereo-stream FTR split: input k holds channels 2k and 2k + 1.
        let stereo = AudioFeed {
            channels_per_input: 2,
            inputs: feed.inputs[..2].to_vec(),
            input_codec: feed.input_codec.clone(),
            trm: None,
            trm_inspection_error: None,
            dm_timeline: None,
            dcr: None,
            merge: None,
            _guards: Vec::new(),
            _channel_dir: None,
        };
        let (args, inner) = stereo.channel_input_args(3, &[]).unwrap();
        assert_eq!(args, ["-f", "aac", "-i", "/tmp/ch1.aac"]);
        assert_eq!(inner, Some(1));
        assert_eq!(stereo.channel_input_args(0, &[]).unwrap().1, Some(0));
        assert!(stereo.channel_input_args(4, &[]).is_none());

        let plain = AudioFeed::plain(PathBuf::from("/in.wav"));
        assert!(!plain.is_merged());
        assert_eq!(
            plain.source_output_args(Some("volume=2")),
            ["-map", "0:a:0", "-af", "volume=2"]
        );
        assert_eq!(plain.source_output_args(None), ["-map", "0:a:0"]);
        assert_eq!(plain.filter_source(), (String::new(), "[0:a:0]".to_string()));
        assert!(plain.channel_input_args(0, &[]).is_none());
    }

    #[test]
    fn ftr_input_uses_native_decoder_and_never_plain_aac() {
        for path in [
            "session.trm",
            "session.ftr",
            "session.tll",
            "SESSION.TRM",
            "SESSION.FTR",
            "SESSION.TLL",
        ] {
            let args = input_codec_args(Path::new(path));
            assert_eq!(args, vec!["-c:a", "ftr"], "unexpected decoder args for {path}");
            assert!(!args.iter().any(|arg| arg.eq_ignore_ascii_case("aac")));
        }

        assert!(input_codec_args(Path::new("ordinary.wav")).is_empty());
        assert_eq!(input_codec_args(Path::new("session.SGMCA")), ["-c:a", "libopus"]);
    }

    #[test]
    fn an_ftr_recording_depoaudio_cannot_read_goes_to_the_native_decoder_when_present() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("unreadable.trm");
        fs::write(&source, b"RIFF\x10\0\0\0AVI LIST\x04\0\0\0junk").unwrap();
        let native = FeedOptions {
            native_ftr: true,
            native_failed: false,
        };
        let feed = prepare_audio_feed_with_cancel(&source, native, None).unwrap();
        assert!(feed.trm().is_none());
        assert!(
            !feed.used_native_ftr(),
            "no layout was read, so there is no split route to retry"
        );
        assert!(feed.trm_inspection_error().unwrap().contains("FTR recording"));
        assert_eq!(feed.input_args(&[])[..2], ["-c:a", "ftr"]);

        // Without the decoder, or once it has failed, the reason is the error.
        assert!(prepare_audio_feed_with_cancel(&source, FeedOptions::default(), None).is_err());
        let failed = FeedOptions {
            native_failed: true,
            ..native
        };
        assert!(prepare_audio_feed_with_cancel(&source, failed, None).is_err());
    }

    #[test]
    fn an_ftr_recording_with_no_round_to_split_goes_to_the_native_decoder_when_present() {
        use crate::trm::tests as trm_fixture;
        let dir = ReservationTestDir::new();
        // Every frame carries the private bit, so DepoAudio's split finds no
        // stream-0 frame; FFmpeg's native decoder does not use the bit.
        let channels = trm_fixture::fake_frames(2, 3);
        let (mut stream, lengths) = trm_fixture::ftr_frame_stream(&channels);
        let mut pos = 0;
        for length in &lengths {
            stream[pos + 2] |= 0x02;
            pos += length;
        }
        let source = dir.path().join("all_private.trm");
        fs::write(
            &source,
            trm_fixture::build_avi(
                &trm_fixture::AviSpec::closed(2),
                &trm_fixture::round_packets(&stream, &lengths, 2, 1),
            ),
        )
        .unwrap();
        assert_eq!(crate::trm::inspect_trm(&source).unwrap().frames_per_channel, 0);
        let native = FeedOptions {
            native_ftr: true,
            native_failed: false,
        };
        let feed = prepare_audio_feed_with_cancel(&source, native, None).unwrap();
        assert!(feed.trm().is_none() && !feed.trm_frames_read());
        assert_eq!(feed.trm_inspection_error(), Some(crate::trm::NO_COMPLETE_ROUND_MESSAGE));
        assert_eq!(
            feed.input_args(&[]),
            ["-c:a", "ftr", "-i", source.to_string_lossy().as_ref()]
        );
        // Without the decoder, or once it has failed, there is nothing to split.
        for options in [
            FeedOptions::default(),
            FeedOptions {
                native_failed: true,
                ..native
            },
        ] {
            assert!(prepare_audio_feed_with_cancel(&source, options, None)
                .unwrap_err()
                .contains("No complete round"));
        }
    }

    #[test]
    fn channel_files_go_to_a_new_private_folder_removed_with_the_feed() {
        use crate::trm::tests as trm_fixture;
        let dir = ReservationTestDir::new();
        let channels = trm_fixture::fake_frames(2, 3);
        let (stream, _) = trm_fixture::ftr_frame_stream(&channels);
        let source = dir.path().join("split.trm");
        fs::write(
            &source,
            trm_fixture::build_avi(
                &trm_fixture::AviSpec::closed(2),
                &trm_fixture::sized_packets(&stream, 37),
            ),
        )
        .unwrap();
        let feed = prepare_audio_feed_with_cancel(&source, FeedOptions::default(), None).unwrap();
        assert_eq!(feed.trm().unwrap().decode_path, crate::trm::DecodePath::Deinterleaved);
        let folder = feed.inputs[0].parent().unwrap().to_path_buf();
        assert!(feed.inputs.iter().all(|input| input.parent() == Some(folder.as_path())));
        assert!(folder
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(CHANNEL_DIR_PREFIX));
        // In this run's private work folder (workdir.rs), not the shared
        // system temporary folder.
        assert!(folder.starts_with(crate::workdir::tests::test_root()));
        assert_eq!(
            crate::workdir::tests::thread_run_files(),
            [folder.file_name().unwrap().to_string_lossy().to_string()]
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&folder), 0o700);
            for input in &feed.inputs {
                assert_eq!(mode(input), 0o600, "{}", input.display());
            }
        }
        // A second conversion gets a folder of its own.
        let other = crate::workdir::new_temp_dir(CHANNEL_DIR_PREFIX).unwrap();
        assert_ne!(other.path(), folder);
        drop(other);
        drop(feed);
        assert!(!folder.exists());
        assert!(crate::workdir::tests::thread_run_files().is_empty());
    }

    #[test]
    fn an_ftr_layout_ftr_itself_refuses_is_not_handed_to_the_native_decoder() {
        use crate::trm::tests as trm_fixture;
        let dir = ReservationTestDir::new();
        let channels = trm_fixture::fake_frames(2, 3);
        let (stream, lengths) = trm_fixture::ftr_frame_stream(&channels);
        let mut spec = trm_fixture::AviSpec::closed(2);
        spec.tag = 0x0180;
        let source = dir.path().join("no_layout_bits.trm");
        fs::write(
            &source,
            trm_fixture::build_avi(&spec, &trm_fixture::round_packets(&stream, &lengths, 2, 1)),
        )
        .unwrap();
        let native = FeedOptions {
            native_ftr: true,
            native_failed: false,
        };
        assert_eq!(
            prepare_audio_feed_with_cancel(&source, native, None).unwrap_err(),
            crate::trm::TRM_UNSPLITTABLE_MESSAGE
        );

        // MP2 audio in an FTR file: FFmpeg reads it as it is, with no
        // decoder forced and no split.
        spec.tag = 0x0050;
        let plain = dir.path().join("mp2.trm");
        fs::write(&plain, trm_fixture::build_avi(&spec, &[vec![0x11; 40]])).unwrap();
        let feed = prepare_audio_feed_with_cancel(&plain, native, None).unwrap();
        assert_eq!(feed.trm().unwrap().decode_path, crate::trm::DecodePath::Plain);
        assert_eq!(feed.input_args(&[]), ["-i", plain.to_string_lossy().as_ref()]);
        assert!(!feed.used_native_ftr() && !feed.trm_frames_read());
        assert_eq!((feed.known_channels(), feed.known_duration()), (None, None));
    }

    #[test]
    fn sgmca_header_strip_rejects_missing_ogg_stream() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("broken.sgmca");
        fs::write(&source, b"vendor header without an ogg stream").unwrap();

        let error = strip_sgmca_header(&source).unwrap_err();

        assert!(error.contains("does not contain an Ogg audio stream"));
    }

    #[test]
    fn sgmca_header_strip_copies_from_the_opus_stream() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("recording.sgmca");
        let stream = crate::sgmca::fixtures::synthetic(5, 312, 0, |_| 30);
        // A stray marker in the prefix must not be taken for the stream.
        let mut bytes = b"vendor-prefix OggS stray".to_vec();
        bytes.extend_from_slice(&stream);
        fs::write(&source, bytes).unwrap();

        let (prepared, guard) = strip_sgmca_header(&source).unwrap();
        assert!(guard.is_some());
        assert!(prepared.starts_with(crate::workdir::tests::test_root()));
        assert_eq!(fs::read(&prepared).unwrap(), stream);
        drop(guard);
        assert!(!prepared.exists(), "the copy goes with its guard");
    }

    #[test]
    fn a_multi_session_sgmca_is_refused_with_or_without_a_header() {
        let dir = ReservationTestDir::new();
        let one = crate::sgmca::fixtures::synthetic(50, 312, 0, |_| 30);
        let segmented = crate::sgmca::fixtures::segmented(&[
            ("2026-11-22T10:15:30-0500", &one),
            ("2026-11-22T10:20:00-0500", &one),
        ]);
        let mut chained = one.clone();
        chained.extend_from_slice(&one);
        for (name, bytes) in [("segmented.sgmca", segmented), ("chained.sgmca", chained)] {
            let source = dir.path().join(name);
            fs::write(&source, bytes).unwrap();
            let error = prepare_audio_feed_with_cancel(&source, FeedOptions::default(), None).unwrap_err();
            assert!(error.contains("2 recording sessions"), "{name}: {error}");
        }
        let entries: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(entries.len(), 2, "no temporary copy is left beside the sources");
    }

    #[test]
    fn plain_sgmca_is_used_in_place() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("plain.sgmca");
        fs::write(&source, crate::sgmca::fixtures::synthetic(5, 312, 0, |_| 30)).unwrap();

        let (prepared, guard) = strip_sgmca_header(&source).unwrap();
        assert!(guard.is_none());
        assert_eq!(prepared, source);
    }

    #[test]
    fn sgmca_header_strip_rejects_a_marker_without_an_opus_header() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("damaged.sgmca");
        fs::write(&source, b"vendor-prefixOggSaudio-payload").unwrap();

        let error = strip_sgmca_header(&source).unwrap_err();
        assert!(error.contains("no readable Opus stream header"), "{error}");
    }

    #[test]
    fn sgmca_header_strip_honors_cancellation_while_copying() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = ReservationTestDir::new();
        let source = dir.path().join("large-recording.sgmca");
        let mut contents = b"vendor-prefix".to_vec();
        contents.extend_from_slice(&crate::sgmca::fixtures::synthetic(5, 312, 0, |_| 30));
        contents.resize(256 * 1024, 7);
        fs::write(&source, contents).unwrap();
        let checks = AtomicUsize::new(0);
        let cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 4;

        let error = prepare_audio_feed_with_cancel(&source, FeedOptions::default(), Some(&cancelled)).unwrap_err();

        assert_eq!(error, AUDIO_PREPARATION_CANCELLED_MESSAGE);
        assert!(checks.load(Ordering::Relaxed) >= 5);
        assert!(
            crate::workdir::tests::thread_run_files().is_empty(),
            "a cancelled copy is removed"
        );
    }

    #[test]
    fn a_dm_is_detected_by_its_header_and_prepared_as_a_temporary_wav() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = ReservationTestDir::new();
        let block = vec![3u8; 520];
        let source = dir.path().join("Session 1.DM");
        let bytes = crate::dm::tests::dm_bytes(
            &crate::dm::tests::gsm_format(),
            520,
            &[(1_763_800_000, &block), (0, &block), (1_763_800_001, &block)],
        );
        fs::write(&source, &bytes).unwrap();
        let format = detect_format_for_path(&source.to_string_lossy()).unwrap();
        assert_eq!(format.key, "digitalcat");
        assert_eq!(format.handler, "digitalcat");
        // A path that cannot be read keeps the extension's answer.
        assert_eq!(detect_format_by_extension("missing.dm").unwrap().key, "digitalcat");
        assert_eq!(
            detect_format_for_path("/no/such/dir/missing.dm").unwrap().key,
            "digitalcat"
        );

        let prepared = prepare_audio_feed_with_cancel(&source, FeedOptions::default(), None).unwrap();
        assert_ne!(prepared.path(), source);
        assert!(prepared.path().starts_with(crate::workdir::tests::test_root()));
        assert_eq!(prepared.path().extension().unwrap(), "wav");
        let wav = fs::read(prepared.path()).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        let timeline = prepared.dm_timeline().cloned().unwrap();
        assert_eq!(timeline.blocks_erased, 1);
        let feed = prepared.path().to_path_buf();
        drop(prepared);
        assert!(!feed.exists(), "the temporary WAV goes with its guard");
        assert_eq!(fs::read(&source).unwrap(), bytes, "the original is only read");

        let checks = AtomicUsize::new(0);
        let cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 2;
        let error = prepare_audio_feed_with_cancel(&source, FeedOptions::default(), Some(&cancelled)).unwrap_err();
        assert_eq!(error, AUDIO_PREPARATION_CANCELLED_MESSAGE);
        assert!(
            crate::workdir::tests::thread_run_files().is_empty(),
            "nothing is left after success or cancellation"
        );

        // A .dm whose every block is recorded over is refused, and leaves
        // nothing behind.
        let erased = dir.path().join("erased.dm");
        fs::write(
            &erased,
            crate::dm::tests::dm_bytes(&crate::dm::tests::gsm_format(), 520, &[(0, &block), (0, &block)]),
        )
        .unwrap();
        let error = prepare_audio_feed_with_cancel(&erased, FeedOptions::default(), None)
            .err()
            .unwrap();
        assert!(error.contains("no audio left"), "{error}");
        assert!(crate::workdir::tests::thread_run_files().is_empty());

        // A copy cut off 300 bytes into its last block still gives the
        // complete blocks before it, and the queue still gets a duration.
        let cut = dir.path().join("Session 1 (cut).dm");
        fs::write(&cut, &bytes[..bytes.len() - 220]).unwrap();
        let prepared = prepare_audio_feed_with_cancel(&cut, FeedOptions::default(), None).unwrap();
        let timeline = prepared.dm_timeline().cloned().unwrap();
        assert_eq!(timeline.blocks_total, 2);
        assert_eq!(timeline.truncated_tail_bytes, 308);
        let wav = fs::read(prepared.path()).unwrap();
        // One kept block (the other is recorded over): 520 bytes of data.
        assert_eq!(
            u32::from_le_bytes(wav[wav.len() - 524..wav.len() - 520].try_into().unwrap()),
            520
        );
        drop(prepared);
        let info = crate::dm::streams_info(&cut).unwrap();
        assert_eq!(info.dm.unwrap().truncated_tail_bytes, 308);
        assert!((info.duration_secs.unwrap() - 2560.0 / 11025.0).abs() < 1e-9);
        assert_eq!(
            fs::read(&cut).unwrap(),
            &bytes[..bytes.len() - 220],
            "the original is only read"
        );
    }

    #[test]
    fn closed_vendor_formats_are_rejected_with_guidance() {
        for (path, key, guidance) in [
            ("dictation.ds2", "ds2", "ODMS"),
            ("DICTATION.DS2", "ds2", "needs its password"),
            ("meeting.arf", "webex", "Webex Recording Converter"),
            ("meeting.wrf", "webex", "WRF2WMV"),
            ("memo.msv", "sony-voice", "Sony Digital Voice Editor"),
            ("memo.DVF", "sony-voice", "Sony Digital Voice Editor"),
        ] {
            let format = detect_format_for_path(path).unwrap_or_else(|| panic!("{path} was not recognised"));
            assert_eq!(format.key, key, "{path}");
            assert_eq!(format.status, "unsupported", "{path}");
            assert_eq!(format.handler, "rejected", "{path}");
            let note = format.note.unwrap();
            assert!(note.contains(guidance), "{path}: {note}");
            assert!(note.contains("cannot be opened here"), "{path}: {note}");
        }
    }

    #[test]
    fn dcr_merge_errors_never_show_a_temporary_path() {
        let windows = b"[out#0/wav @ 0x1] Error closing file C:/Users/Jane Roe/AppData/Local/Temp/depoaudio_ab12.wav: No space left on device\n";
        let message = dcr_merge_error("IMA ADPCM", windows);
        assert_eq!(
            message,
            "FFmpeg could not decode this recording's IMA ADPCM audio. FFmpeg said: No space left on device"
        );
        let backslashes = br"C:\Users\Jane Roe\AppData\Local\Temp\depoaudio_ab12.wav: Invalid data found";
        assert!(!dcr_merge_error("GSM 6.10", backslashes).contains("Jane"));
        // A path with no plain reason after it is left out entirely.
        let message = dcr_merge_error("MP3", b"/home/jane/tmp/depoaudio_1.wav");
        assert_eq!(message, "FFmpeg could not decode this recording's MP3 audio.");
        let message = dcr_merge_error("MP3", b"\n[mp3float @ 0x2] Header missing\n\n");
        assert!(
            message.ends_with("FFmpeg said: [mp3float @ 0x2] Header missing"),
            "{message}"
        );
    }

    #[test]
    fn dcr_routes_by_content_to_the_reader_or_neutral_guidance() {
        let dir = ReservationTestDir::new();
        let liberty = dir.path().join("hearing.dcr");
        fs::write(&liberty, b"HGCRLCRS\x15\0\0\0\0\0\0\0rest").unwrap();
        let reader = detect_format_for_path(&liberty.to_string_lossy()).unwrap();
        assert_eq!(reader.key, "dcr");
        assert_eq!(reader.handler, "dcr");
        assert_eq!(reader.status, "experimental");
        assert_eq!(reader.vendor, "Liberty / BIS Digital");
        assert!(reader.note.unwrap().contains("synthetic files only"));

        let other = dir.path().join("other.dcr");
        fs::write(&other, b"\x00\x01court-audio").unwrap();
        let dcr = detect_format_for_path(&other.to_string_lossy()).unwrap();
        assert_eq!(dcr.key, "dcr-unknown");
        assert_eq!(dcr.handler, "rejected");
        let note = dcr.note.unwrap();
        assert!(note.contains("software that recorded it"), "{note}");
        assert!(
            note.contains("Liberty Court Recorder") && note.contains("BIS Digital"),
            "{note}"
        );
        assert!(!dcr.vendor.contains("High Criteria"));
    }

    #[test]
    fn dcr_that_is_a_tiff_camera_image_is_called_out() {
        let dir = ReservationTestDir::new();
        for (name, bytes) in [
            ("little.dcr", &b"II*\0rest-of-tiff"[..]),
            ("big.DCR", &b"MM\0*rest-of-tiff"[..]),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            let format = detect_format_for_path(&path.to_string_lossy()).unwrap();
            assert_eq!(format.key, "dcr-image", "{name}");
            assert_eq!(format.handler, "rejected");
            assert!(format
                .note
                .unwrap()
                .contains("starts like a camera raw image (TIFF), not like court audio"));
        }

        // A DCR without either signature, a short file and an empty file get
        // the neutral export guidance.
        for (name, bytes) in [
            ("court.dcr", &b"\x00\x01court-audio"[..]),
            ("short.dcr", &b"II"[..]),
            ("empty.dcr", &b""[..]),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                detect_format_for_path(&path.to_string_lossy()).unwrap().key,
                "dcr-unknown",
                "{name}"
            );
        }
    }

    #[test]
    fn rejected_formats_never_reach_conversion_handlers() {
        for format in get_formats()
            .into_iter()
            .filter(|format| format.status == "unsupported")
        {
            assert_eq!(format.handler, "rejected", "{}", format.key);
            assert!(
                format.note.as_deref().is_some_and(|note| !note.is_empty()),
                "{}",
                format.key
            );
        }
    }

    #[test]
    fn legacy_legal_video_containers_extract_audio_like_mp4() {
        for path in [
            "depo.mpg",
            "DEPO.MPEG",
            "VTS_01_1.VOB",
            "hearing.ts",
            "00001.MTS",
            "00001.m2ts",
            "depo.wmv",
            "depo.asf",
        ] {
            let format = detect_format_for_path(path).unwrap_or_else(|| panic!("{path} was not recognised"));
            assert_eq!(format.key, "video", "{path}");
            assert_eq!(format.handler, "passthrough", "{path}");
            assert_eq!(format.status, "supported", "{path}");
            assert!(input_codec_args(Path::new(path)).is_empty(), "{path}");
        }
    }

    #[test]
    fn ts_files_must_look_like_a_transport_stream_when_readable() {
        let dir = ReservationTestDir::new();
        let mut ts = vec![0u8; 188 * 2];
        ts[0] = 0x47;
        ts[188] = 0x47;
        let mut m2ts = vec![0u8; 192 * 2];
        m2ts[4] = 0x47;
        m2ts[196] = 0x47;
        let mut broken_stride = vec![0u8; 188 * 2];
        broken_stride[0] = 0x47;
        let cases: [(&str, &[u8], bool); 6] = [
            ("broadcast.ts", &ts, true),
            ("avchd.ts", &m2ts, true),
            ("tiny.ts", &[0x47], true),
            ("component.ts", b"export const value = 1\n", false),
            ("Greeting.ts", &broken_stride, false),
            ("empty.ts", b"", false),
        ];
        for (name, bytes, video) in cases {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            let detected = detect_format_for_path(&path.to_string_lossy());
            assert_eq!(detected.map(|f| f.key), video.then(|| "video".to_string()), "{name}");
        }
        // The extension decides when the file cannot be read.
        assert_eq!(detect_format_for_path("missing/hearing.ts").unwrap().key, "video");
    }

    /// Packets of `stride` bytes whose sync byte sits at `sync_at` within each
    /// packet, with the first `skip` bytes cut off (a capture cut mid-packet).
    fn transport_stream_bytes(stride: usize, sync_at: usize, packets: usize, skip: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; stride * packets];
        for packet in 0..packets {
            bytes[packet * stride + sync_at] = 0x47;
        }
        bytes.split_off(skip)
    }

    #[test]
    fn ts_files_cut_mid_packet_or_with_a_short_header_are_still_transport_streams() {
        let dir = ReservationTestDir::new();
        let mut padded = vec![0x20u8; 32];
        padded.extend(transport_stream_bytes(188, 0, 8, 0));
        let mut too_few = vec![0u8; 100];
        too_few.extend(transport_stream_bytes(188, 0, TS_RESYNC_PACKETS - 1, 0));
        let mut prose = Vec::new();
        while prose.len() < TS_SCAN_BYTES {
            prose.extend_from_slice(b"import { Greeting } from './Greeting'\nexport const GREETING = 'Good morning'\n");
        }
        let cases: Vec<(&str, Vec<u8>, bool)> = vec![
            ("cut.ts", transport_stream_bytes(188, 0, 40, 100), true),
            ("cut-m2ts.ts", transport_stream_bytes(192, 4, 40, 1), true),
            ("padded.ts", padded, true),
            // Too few packets in a row after a misaligned start.
            ("too-few.ts", too_few, false),
            // Source text with a capital G here and there.
            ("greeting.ts", prose, false),
        ];
        for (name, bytes, video) in cases {
            let path = dir.path().join(name);
            fs::write(&path, &bytes).unwrap();
            let detected = detect_format_for_path(&path.to_string_lossy());
            assert_eq!(detected.map(|f| f.key), video.then(|| "video".to_string()), "{name}");
        }
    }

    /// Opening a FIFO for reading blocks until a writer appears; detection
    /// must not open one, so a pipe named *.ts or *.dcr returns at once with
    /// the extension's answer.
    #[cfg(unix)]
    #[test]
    fn content_checks_never_open_a_fifo() {
        let dir = ReservationTestDir::new();
        for (name, key) in [("pipe.ts", "video"), ("pipe.dcr", "dcr")] {
            let path = dir.path().join(name);
            let status = std::process::Command::new("mkfifo").arg(&path).status();
            if !status.is_ok_and(|status| status.success()) {
                eprintln!("skipping: mkfifo is not available");
                return;
            }
            let (sender, receiver) = std::sync::mpsc::channel();
            let probe = path.to_string_lossy().to_string();
            std::thread::spawn(move || {
                let _ = sender.send(detect_format_for_path(&probe).map(|f| f.key));
            });
            let detected = receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap_or_else(|_| panic!("{name}: detection blocked on a FIFO"));
            assert_eq!(detected.as_deref(), Some(key), "{name}");
        }
    }

    #[test]
    fn ftr_codec_hint_comes_from_the_extension_alone() {
        assert_eq!(input_codec_args(Path::new("session.trm")), vec!["-c:a", "ftr"]);
        assert_eq!(input_codec_args(Path::new("missing/SESSION.FTR")), vec!["-c:a", "ftr"]);
        assert_eq!(detect_format_by_extension("hearing.ts").unwrap().key, "video");
        assert_eq!(detect_format_by_extension("hearing.dcr").unwrap().key, "dcr");
        assert!(detect_format_by_extension("notes.txt").is_none());
    }

    #[test]
    fn every_output_drops_video_and_subtitle_streams() {
        for format in ["wav", "mp3", "flac", "opus", "m4a"] {
            let args = output_args(format, "48000", 192);
            assert_eq!(&args[..2], ["-vn", "-sn"], "{format}");
        }
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
    fn concurrent_output_reservations_are_distinct_and_exist() {
        const WORKERS: usize = 16;
        let dir = ReservationTestDir::new();
        let requested = Arc::new(dir.path().join("hearing.wav"));
        let barrier = Arc::new(Barrier::new(WORKERS));

        let handles: Vec<_> = (0..WORKERS)
            .map(|_| {
                let requested = Arc::clone(&requested);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    reserve_unique_path(requested.as_ref().as_path())
                })
            })
            .collect();

        let reserved: Vec<PathBuf> = handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .expect("reservation worker panicked")
                    .expect("reserve output")
            })
            .collect();
        let distinct: HashSet<PathBuf> = reserved.iter().cloned().collect();

        assert_eq!(distinct.len(), WORKERS);
        assert!(reserved.iter().all(|path| path.is_file()));
        assert!(reserved.iter().all(|path| fs::metadata(path).unwrap().len() == 0));
    }

    #[test]
    fn reservation_preserves_an_existing_output() {
        let dir = ReservationTestDir::new();
        let requested = dir.path().join("hearing.wav");
        fs::write(&requested, b"existing recording").unwrap();

        let reserved = reserve_unique_path(&requested).unwrap();

        assert_eq!(
            reserved.file_name().and_then(|name| name.to_str()),
            Some("hearing_1.wav")
        );
        assert_eq!(fs::read(&requested).unwrap(), b"existing recording");
        assert_eq!(fs::metadata(reserved).unwrap().len(), 0);
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
    fn wav_output_switches_to_rf64_only_when_needed() {
        assert_eq!(
            output_args("wav", "48000", 192),
            vec!["-vn", "-sn", "-acodec", "pcm_s16le", "-ar", "48000"]
        );
        for format in ["wav", "mp3", "flac", "opus", "m4a"] {
            assert!(
                !output_args(format, "48000", 192).contains(&"-rf64".to_string()),
                "{format}"
            );
        }
        // A one-hour stereo 48 kHz WAV is about 0.6 GiB: plain RIFF.
        let hour_stereo = wav_estimated_bytes(Some(3600.0), Some(2), Some(48_000));
        assert_eq!(hour_stereo, Some(691_200_000));
        assert!(wav_rf64_args(hour_stereo).is_empty());
        // 16 channels at 48 kHz pass 3.5 GiB after about 41 minutes.
        assert!(wav_rf64_args(wav_estimated_bytes(Some(40.0 * 60.0), Some(16), Some(48_000))).is_empty());
        assert_eq!(
            wav_rf64_args(wav_estimated_bytes(Some(46.0 * 60.0), Some(16), Some(48_000))),
            vec!["-rf64", "auto"]
        );
        assert_eq!(wav_rf64_args(Some(RF64_ESTIMATE_BYTES - 1)), Vec::<String>::new());
        assert_eq!(wav_rf64_args(Some(RF64_ESTIMATE_BYTES)), vec!["-rf64", "auto"]);
        // A size that cannot be estimated keeps the 4 GiB safety.
        for unknown in [
            wav_estimated_bytes(None, Some(2), Some(48_000)),
            wav_estimated_bytes(Some(60.0), None, Some(48_000)),
            wav_estimated_bytes(Some(f64::NAN), Some(2), Some(48_000)),
        ] {
            assert_eq!(unknown, None);
            assert_eq!(wav_rf64_args(unknown), vec!["-rf64", "auto"]);
        }
    }

    #[test]
    fn every_sidecar_reads_the_same_audio_stream() {
        assert_eq!(SOURCE_AUDIO_STREAM, 0);
        assert_eq!(source_audio_stream_specifier(), "a:0");
        assert_eq!(source_audio_map_args(), vec!["-map", "0:a:0"]);
        assert_eq!(source_audio_filter_input(), "[0:a:0]");
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
