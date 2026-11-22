use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use std::sync::{Arc, OnceLock};

use regex::Regex;
use uuid::Uuid;

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
// - .dcr: a closed court-recorder format with no public documentation; the
//   recorder software is named only as an example.
const DCR_NOTE: &str = "DCR recordings use a closed format. Export to WAV from the software that recorded it (for example Liberty Court Recorder or BIS Digital), then add the WAV.";
const DCR_IMAGE_NOTE: &str = "This .dcr starts like a camera raw image (TIFF), not like court audio. Check that you picked the right file. If it is a court recording, export it to WAV from the software that recorded it.";
const DS2_NOTE: &str = "DS2 (DSS Pro) dictation files cannot be opened here. Export to WAV with OM System / Olympus dictation software (for example ODMS), then add the WAV. An encrypted DS2 also needs its password in that software.";
const WEBEX_NOTE: &str = "Webex network recordings (.arf, .wrf) cannot be opened here. Convert an .arf to MP4 with the Webex Recording Converter, or a .wrf to WMV with the Webex WRF2WMV Converter (both Windows only), then add the converted file.";
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
            note: Some("FTR interleaves one AAC stream per microphone. DepoAudio reads a file with FFmpeg's native FTR decoder when its layout allows, and otherwise splits the channels itself first; the export record says which. This reading is based on interoperability notes and has not yet been checked against a real FTR recording. Add all .trm or .tll files of a session together.".into()) },
        FormatInfo { key: "bwf".into(), name: "Broadcast WAV".into(), vendor: "CourtSmart / Various".into(),
            status: "supported".into(), handler: "passthrough".into(), channels: None, note: None },
        FormatInfo { key: "digitalcat".into(), name: "DigitalCAT Audio".into(), vendor: "Stenovations".into(),
            status: "experimental".into(), handler: "passthrough".into(), channels: None,
            note: Some("No public spec — conversion may fail. Please report results on GitHub.".into()) },
        FormatInfo { key: "aes".into(), name: "Eclipse AudioSync".into(), vendor: "Eclipse CAT".into(),
            status: "unsupported".into(), handler: "rejected".into(), channels: None,
            note: Some("AES-128 encrypted. Open in Eclipse → File → Export Audio → WAV first.".into()) },
        // Liberty Court Recorder and BIS Digital both write .dcr, and their
        // files do not open in each other's software, so the guidance names
        // neither as the only source.
        FormatInfo { key: "dcr".into(), name: "Digital Court Recording (DCR)".into(), vendor: "Liberty / BIS Digital".into(),
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

/// The format of a file: the extension decides, then two cheap content
/// checks. A `.dcr` that starts like a TIFF is a camera raw image, and a
/// readable `.ts` with no transport-stream sync pattern (most often a
/// TypeScript source file) is left unrecognised. Both checks read only a
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
        "dcr" if looks_like_tiff(Path::new(path)) => get_formats().into_iter().find(|f| f.key == "dcr-image"),
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

/// True when the file starts with a TIFF header (`II*\0` or `MM\0*`), which is
/// what a Kodak DCR camera raw image is. Liberty and BIS do not publish their
/// DCR layout, so this is not checked against a real court DCR file; a court
/// DCR that did start with these bytes would get the camera-image note, which
/// still says how to export a court recording.
fn looks_like_tiff(path: &Path) -> bool {
    matches!(
        leading_bytes::<4>(path),
        Some((bytes, 4)) if *bytes == *b"II*\0" || *bytes == *b"MM\0*"
    )
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
) -> Result<(PathBuf, bool), String> {
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
    if offset == 0 {
        return Ok((src.to_path_buf(), false));
    }

    // Security note: UUID-based temp filenames are unpredictable, which is sufficient
    // for a single-user desktop app. The system temp dir inherits OS-level permissions
    // (typically user-only on macOS/Windows). For multi-user or server contexts, consider
    // creating a private subdirectory with restrictive permissions (0o700).
    // The copy keeps the .sgmca extension so every later step treats it as
    // SGMCA (decoder choice included); FFmpeg finds the Ogg stream by content.
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_{}.sgmca",
        Uuid::new_v4().to_string().replace('-', "")
    ));
    file.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let copy_result = (|| -> Result<(), String> {
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|error| error.to_string())?;
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
        out.sync_all().map_err(|error| error.to_string())
    })();
    if let Err(error) = copy_result {
        let _ = fs::remove_file(&tmp);
        return Err(error.to_string());
    }
    Ok((tmp, true))
}

#[cfg(test)]
pub(crate) fn strip_sgmca_header(src: &Path) -> Result<(PathBuf, bool), String> {
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
/// SGMCA's Ogg stream in a temporary copy; or, for an FTR recording that
/// FFmpeg's native decoder cannot take as it is, one temporary mono ADTS file
/// per channel, joined back in channel order with `amerge`. Temporary files
/// live exactly as long as the feed.
#[derive(Debug)]
pub(crate) struct AudioFeed {
    inputs: Vec<PathBuf>,
    /// Options placed before every `-i` (a decoder or demuxer choice).
    input_codec: Vec<String>,
    trm: Option<crate::trm::TrmRoute>,
    _guards: Vec<crate::safety::TempFile>,
}

impl AudioFeed {
    /// The file itself, read with FFmpeg's own choices.
    pub(crate) fn plain(path: PathBuf) -> Self {
        Self {
            inputs: vec![path],
            input_codec: Vec::new(),
            trm: None,
            _guards: Vec::new(),
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

    /// The arguments for one channel's own input, when the feed has one
    /// input per channel (so a per-channel pass decodes only that channel).
    pub(crate) fn channel_input_args(&self, channel: u32, per_input: &[String]) -> Option<Vec<String>> {
        if !self.is_merged() {
            return None;
        }
        let input = self.inputs.get(channel as usize)?;
        let mut args: Vec<String> = per_input.to_vec();
        args.extend(self.input_codec.iter().cloned());
        args.extend(["-i".into(), input.to_string_lossy().to_string()]);
        Some(args)
    }

    /// `[0:a:0][1:a:0]…amerge=inputs=N`: channel k of the joined stream is
    /// input k's only channel.
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

    /// Channel count read from the recording itself (FTR), if any.
    pub(crate) fn known_channels(&self) -> Option<u32> {
        self.trm.as_ref().map(|route| u32::from(route.info.channels))
    }

    /// Duration from the frames actually read (FTR): never from a header
    /// length an unclosed recording leaves at its placeholder.
    pub(crate) fn known_duration(&self) -> Option<f64> {
        self.trm.as_ref().and_then(|route| route.info.duration_secs)
    }

    /// FFmpeg reads this FTR recording with its native decoder.
    pub(crate) fn used_native_ftr(&self) -> bool {
        self.trm
            .as_ref()
            .is_some_and(|route| route.decode_path == crate::trm::DecodePath::NativeFtr)
    }

    pub(crate) fn known_sample_rate(&self) -> Option<u32> {
        self.trm.as_ref().and_then(|route| route.info.sample_rate)
    }
}

/// The selected file is an FTR recording (`.trm`, `.ftr`, `.tll`).
pub(crate) fn is_trm_path(path: &Path) -> bool {
    detect_format_by_extension(&path.to_string_lossy()).is_some_and(|format| format.handler == "ftr")
}

fn prepare_trm_feed(
    src: &Path,
    options: FeedOptions,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<AudioFeed, String> {
    let info = crate::trm::inspect_trm_with_cancel(src, cancelled)?;
    let (decode_path, reasons) = crate::trm::choose_route(&info, options);
    match decode_path {
        crate::trm::DecodePath::NativeFtr => Ok(AudioFeed {
            inputs: vec![src.to_path_buf()],
            input_codec: input_codec_args(src),
            trm: Some(crate::trm::TrmRoute {
                info,
                decode_path,
                route_reasons: reasons,
            }),
            _guards: Vec::new(),
        }),
        crate::trm::DecodePath::Deinterleaved => {
            let (written, files) =
                crate::trm::deinterleave_trm_with_cancel(src, &std::env::temp_dir(), Some(&info), cancelled)?;
            let guards = files.iter().cloned().map(crate::safety::TempFile::new).collect();
            Ok(AudioFeed {
                inputs: files,
                input_codec: vec!["-f".into(), "aac".into()],
                trm: Some(crate::trm::TrmRoute {
                    info: written,
                    decode_path,
                    route_reasons: reasons,
                }),
                _guards: guards,
            })
        }
    }
}

pub(crate) fn prepare_audio_feed_with_cancel(
    src: &Path,
    options: FeedOptions,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<AudioFeed, String> {
    if cancelled.is_some_and(|check| check()) {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    if is_trm_path(src) {
        return prepare_trm_feed(src, options, cancelled);
    }
    let is_sgmca = src
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sgmca"));
    if !is_sgmca {
        return Ok(AudioFeed::plain(src.to_path_buf()));
    }

    let (feed, is_temp) = strip_sgmca_header_with_cancel(src, cancelled)?;
    let mut prepared = AudioFeed::plain(feed.clone());
    if is_temp {
        prepared._guards.push(crate::safety::TempFile::new(feed));
    }
    Ok(prepared)
}

/// Prepare a feed without blocking the async command executor. The
/// cancellation predicate is checked between every 64 KiB read or copied and
/// owns no borrowed command state, so scan/conversion/waveform callers can
/// reuse it. For an FTR recording this asks the bundled FFmpeg (once per
/// process) whether it has the native `ftr` decoder.
pub(crate) async fn prepare_audio_feed_cancellable(
    app: &tauri::AppHandle,
    src: PathBuf,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<AudioFeed, String> {
    let options = crate::ffmpeg::feed_options(app, &src).await;
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
    src: PathBuf,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<AudioFeed, String> {
    let options = FeedOptions {
        native_ftr: true,
        native_failed: true,
    };
    tauri::async_runtime::spawn_blocking(move || {
        prepare_audio_feed_with_cancel(&src, options, Some(cancelled.as_ref()))
    })
    .await
    .map_err(|error| format!("Audio preparation task failed: {error}"))?
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
            _guards: Vec::new(),
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
        assert_eq!(
            feed.channel_input_args(2, &[]).unwrap(),
            ["-f", "aac", "-i", "/tmp/ch2.aac"]
        );
        assert!(feed.channel_input_args(3, &[]).is_none());

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

        let (prepared, is_temp) = strip_sgmca_header(&source).unwrap();
        assert!(is_temp);
        assert_eq!(fs::read(&prepared).unwrap(), stream);
        fs::remove_file(prepared).unwrap();
    }

    #[test]
    fn plain_sgmca_is_used_in_place() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("plain.sgmca");
        fs::write(&source, crate::sgmca::fixtures::synthetic(5, 312, 0, |_| 30)).unwrap();

        let (prepared, is_temp) = strip_sgmca_header(&source).unwrap();
        assert!(!is_temp);
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
    fn dcr_guidance_names_both_recorders_without_assuming_one() {
        let dcr = detect_format_for_path("hearing.dcr").unwrap();
        assert_eq!(dcr.key, "dcr");
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

        // A court-recorder DCR (not TIFF), a short file and an empty file keep
        // the court guidance.
        for (name, bytes) in [
            ("court.dcr", &b"\x00\x01court-audio"[..]),
            ("short.dcr", &b"II"[..]),
            ("empty.dcr", &b""[..]),
        ] {
            let path = dir.path().join(name);
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                detect_format_for_path(&path.to_string_lossy()).unwrap().key,
                "dcr",
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
