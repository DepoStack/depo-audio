use std::fs;
use std::io::{Read, Write};
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
// - .dcr: a closed court-recorder format with no public documentation; the
//   recorder software is named only as an example.
const DCR_NOTE: &str = "DCR recordings use a closed format. Export to WAV from the software that recorded it (for example Liberty Court Recorder or BIS Digital), then add the WAV.";
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
            note: Some("Direct Audio: a file header, then one Ogg Opus stream per recording session, each with one mono stream per channel. Sessions are joined in order.".into()) },
        FormatInfo { key: "ftr".into(), name: "FTR Recording".into(), vendor: "For The Record".into(),
            status: "experimental".into(), handler: "ftr".into(), channels: Some("4–16".into()),
            note: Some("FTR uses its own multichannel codec (tag 0x4180) and requires FFmpeg's native FTR decoder. Drop all .trm files for a session together.".into()) },
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

/// Input-decoder arguments that must appear before `-i`.
///
/// FTR packets contain one modified AAC unit per court channel. FFmpeg's native
/// `ftr` decoder unwraps and repairs those units before delegating to AAC; using
/// the ordinary `aac` decoder directly fails on real recordings and must never
/// be used as a fallback.
///
/// SGMCA streams are decoded with libopus, the reference Opus decoder. An
/// SGMCA concat script (see `prepare_sgmca_feed`) also gets the concat
/// demuxer options from `feed_format_args`, which must come before `-i`.
/// FFmpeg's built-in Opus decoder has been seen to corrupt a channel of a
/// 16 kHz mapping-family-255 stream (one channel decoding to full-scale
/// noise) that libopus decodes cleanly; every bundled FFmpeg build includes
/// libopus.
pub(crate) fn input_codec_args(path: &Path) -> Vec<String> {
    // An SGMCA concat script: the demuxer options, then libopus.
    if is_sgmca_concat_feed(path) {
        let mut args = feed_format_args(path);
        args.extend(["-c:a".into(), "libopus".into()]);
        args.extend(sgmca_feed_start_args(path));
        return args;
    }
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
        "trm" | "ftr" => "ftr",
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

// ── SGMCA input ──────────────────────────────────────────────────────────────

pub(crate) const AUDIO_PREPARATION_CANCELLED_MESSAGE: &str = "Audio preparation cancelled";

/// File-name prefix and extension of the concat script an SGMCA feed uses.
const SGMCA_FEED_PREFIX: &str = "depoaudio_sgmca_";
const SGMCA_FEED_EXTENSION: &str = "ffconcat";

/// True for the concat script `prepare_sgmca_feed` writes.
fn is_sgmca_concat_feed(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(SGMCA_FEED_EXTENSION))
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(SGMCA_FEED_PREFIX))
}

/// The marker before the first session's pre-skip in a concat script's name.
const SGMCA_FEED_LEAD_MARKER: &str = "_lead";

/// The first session's Opus pre-skip, from the name of the concat script
/// `prepare_sgmca_feed` wrote. None for any other path.
fn sgmca_feed_lead(path: &Path) -> Option<u16> {
    if !is_sgmca_concat_feed(path) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    let (_, lead) = stem.rsplit_once(SGMCA_FEED_LEAD_MARKER)?;
    lead.parse().ok()
}

/// `-itsoffset` for an SGMCA concat script, so its audio starts at zero as a
/// bare Ogg file does.
///
/// FFmpeg's Ogg demuxer gives a bare Opus stream a start time that already
/// allows for the pre-skip, so its first decoded sample lands at zero. The
/// concat demuxer takes the first packet's timestamp instead, which is the
/// pre-skip *before* zero, and FFmpeg then moves the whole input later by
/// that much. WAV, FLAC, MP3 and Ogg outputs drop the gap, but MP4 (M4A)
/// keeps it as a lead-in: every sample came out 312 samples (6.5 ms) late
/// and the file grew by as much. This moves the input back by exactly the
/// pre-skip, so the first decoded sample is at zero for every output.
fn sgmca_feed_start_args(path: &Path) -> Vec<String> {
    match sgmca_feed_lead(path) {
        Some(lead) if lead > 0 => vec![
            "-itsoffset".into(),
            // Microseconds: within 0.05 of a sample, and FFmpeg rounds to the
            // nearest sample at 48 kHz.
            format!("-{}us", (u64::from(lead) * 1_000_000 + 24_000) / 48_000),
        ],
        _ => Vec::new(),
    }
}

/// Input options (before `-i`) that every ffmpeg and ffprobe call needs to
/// read `feed`. For an SGMCA concat script: the concat demuxer, `-safe 0`
/// (its entries are `subfile` URLs into the original file) and a protocol
/// whitelist of `file` and `subfile` only. Nothing for any other feed.
pub(crate) fn feed_format_args(feed: &Path) -> Vec<String> {
    if is_sgmca_concat_feed(feed) {
        vec![
            "-protocol_whitelist".into(),
            "file,subfile".into(),
            "-f".into(),
            "concat".into(),
            "-safe".into(),
            "0".into(),
        ]
    } else {
        Vec::new()
    }
}

/// Read an SGMCA file's layout and give FFmpeg a way to read its audio.
///
/// A Direct Audio file holds a header and one Ogg Opus stream per recording
/// session. FFmpeg gets a small concat script (a temporary text file of a
/// few hundred bytes, removed with the returned guard) that plays each
/// session's byte range of the original file in file order through the
/// `subfile` protocol, so no copy of the audio is made and the original is
/// only read. A bare Ogg file (the older layout) is used in place. The full
/// page walk checks every CRC before FFmpeg is started, so damage is named
/// with its segment and byte.
pub(crate) fn prepare_sgmca_feed(
    src: &Path,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<(PathBuf, Option<crate::safety::TempFile>, crate::sgmca::SgmcaInfo), String> {
    let is_cancelled = || cancelled.is_some_and(|check| check());
    if is_cancelled() {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    let info = crate::sgmca::inspect(src, cancelled).map_err(|error| {
        if error == crate::sgmca::INSPECT_CANCELLED_MESSAGE {
            AUDIO_PREPARATION_CANCELLED_MESSAGE.to_string()
        } else {
            error
        }
    })?;
    if is_cancelled() {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    if info.container == crate::sgmca::Container::BareOgg {
        return Ok((src.to_path_buf(), None, info));
    }
    let script = crate::sgmca::concat_script(src, &info)?;
    // The first session FFmpeg plays carries the pre-skip the input must be
    // moved back by (see `sgmca_feed_start_args`).
    let lead = info
        .segments
        .iter()
        .find(|segment| segment.samples > 0)
        .map_or(0, |segment| segment.head.pre_skip);
    // The script goes in DepoAudio's private work folder (workdir.rs), like
    // every other temporary file a conversion reads. It names only byte
    // ranges of the file the person selected, and its name carries the
    // pre-skip after the identifier (`<prefix><uuid>_lead<n>.ffconcat`).
    let (guard, mut file) = crate::workdir::new_temp_file_tagged(
        SGMCA_FEED_PREFIX,
        &format!("{SGMCA_FEED_LEAD_MARKER}{lead}"),
        SGMCA_FEED_EXTENSION,
    )
    .map_err(|error| format!("Could not prepare the SGMCA sessions for FFmpeg: {error}"))?;
    // Read back by this run only: no sync_all.
    file.write_all(script.as_bytes())
        .map_err(|error| format!("Could not prepare the SGMCA sessions for FFmpeg: {error}"))?;
    Ok((guard.path.clone(), Some(guard), info))
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

/// What FFmpeg reads for a selected file, and what preparing it found.
pub(crate) struct PreparedFeed {
    pub path: PathBuf,
    /// The temporary copy, when there is one; removed when this drops.
    pub guard: Option<crate::safety::TempFile>,
    /// For a `.dm`: its blocks and time stamps, for the export record.
    pub dm_timeline: Option<crate::dm::DmTimeline>,
}

fn has_extension(src: &Path, wanted: &str) -> bool {
    src.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(wanted))
}

/// Prepare the selected file for FFmpeg: an SGMCA gets a concat script over
/// its recording sessions (`prepare_sgmca_feed`) and a `.dm` has its audio
/// read into a WAV, each a temporary file in the private work folder that
/// every probe, scan, waveform and conversion reads; any other file is used
/// as it is.
pub(crate) fn prepare_audio_feed_detailed(
    src: &Path,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<PreparedFeed, String> {
    if cancelled.is_some_and(|check| check()) {
        return Err(AUDIO_PREPARATION_CANCELLED_MESSAGE.into());
    }
    if has_extension(src, "dm") {
        let (guard, timeline) = demux_dm_with_cancel(src, cancelled)?;
        return Ok(PreparedFeed {
            path: guard.path.clone(),
            guard: Some(guard),
            dm_timeline: Some(timeline),
        });
    }
    if !has_extension(src, "sgmca") {
        return Ok(PreparedFeed {
            path: src.to_path_buf(),
            guard: None,
            dm_timeline: None,
        });
    }

    let (feed, guard, _) = prepare_sgmca_feed(src, cancelled)?;
    Ok(PreparedFeed {
        path: feed,
        guard,
        dm_timeline: None,
    })
}

pub(crate) fn prepare_audio_feed_with_cancel(
    src: &Path,
    cancelled: Option<&(dyn Fn() -> bool + Send + Sync)>,
) -> Result<(PathBuf, Option<crate::safety::TempFile>), String> {
    let prepared = prepare_audio_feed_detailed(src, cancelled)?;
    Ok((prepared.path, prepared.guard))
}

/// Prepare an SGMCA feed without blocking the async command executor. The
/// cancellation predicate is checked between every 64 KiB copied and owns no
/// borrowed command state, so scan/conversion/waveform callers can reuse it.
pub(crate) async fn prepare_audio_feed_cancellable(
    src: PathBuf,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(PathBuf, Option<crate::safety::TempFile>), String> {
    tauri::async_runtime::spawn_blocking(move || prepare_audio_feed_with_cancel(&src, Some(cancelled.as_ref())))
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
        // per channel (mapping family 255), encoded at 48 kHz as the notes
        // say CATalyst's own encoder runs (so the input rate tag is 48000). FFmpeg
        // writes the bare Ogg stream; the conversion then adds the Direct
        // Audio file and segment headers (`sgmca::write_direct_audio`).
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
            "48000".into(),
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
    }

    #[test]
    fn ftr_input_uses_native_decoder_and_never_plain_aac() {
        for path in ["session.trm", "session.ftr", "SESSION.TRM", "SESSION.FTR"] {
            let args = input_codec_args(Path::new(path));
            assert_eq!(args, vec!["-c:a", "ftr"], "unexpected decoder args for {path}");
            assert!(!args.iter().any(|arg| arg.eq_ignore_ascii_case("aac")));
        }

        assert!(input_codec_args(Path::new("ordinary.wav")).is_empty());
        assert_eq!(input_codec_args(Path::new("session.SGMCA")), ["-c:a", "libopus"]);
    }

    #[test]
    fn sgmca_feed_refuses_a_file_without_the_direct_audio_header() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("broken.sgmca");
        fs::write(&source, b"vendor header without an ogg stream, padded to forty bytes").unwrap();

        let error = prepare_audio_feed_with_cancel(&source, None).unwrap_err();

        assert!(error.contains("not a Direct Audio file"), "{error}");
    }

    #[test]
    fn direct_audio_sgmca_is_read_in_place_through_a_concat_script() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("recording.sgmca");
        let bytes = crate::sgmca::fixtures::synthetic_direct(5, 312, 0, |_| 30);
        fs::write(&source, &bytes).unwrap();

        let (feed, guard, info) = prepare_sgmca_feed(&source, None).unwrap();
        assert!(guard.is_some());
        assert!(is_sgmca_concat_feed(&feed));
        assert!(
            feed.starts_with(crate::workdir::tests::test_root()),
            "the script goes in the private work folder"
        );
        let script = fs::read_to_string(&feed).unwrap();
        assert!(script.starts_with("ffconcat version 1.0\n"));
        assert!(
            script.contains(&format!("subfile,,start,96,end,{},,:file:", bytes.len())),
            "{script}"
        );
        assert_eq!(info.segments.len(), 1);
        assert_eq!(
            input_codec_args(&feed),
            [
                "-protocol_whitelist",
                "file,subfile",
                "-f",
                "concat",
                "-safe",
                "0",
                "-c:a",
                "libopus",
                "-itsoffset",
                "-6500us"
            ]
        );
        assert_eq!(sgmca_feed_lead(&feed), Some(312));
        assert_eq!(feed_format_args(&feed)[..2], ["-protocol_whitelist", "file,subfile"]);
        assert!(feed_format_args(&source).is_empty());
        // The original is never touched, and the script goes with its guard.
        assert_eq!(fs::read(&source).unwrap(), bytes);
        drop(guard);
        assert!(!feed.exists());
    }

    #[test]
    fn an_sgmca_concat_feed_is_moved_back_by_exactly_its_first_pre_skip() {
        let feed = |name: &str| std::env::temp_dir().join(name);
        let args = |name: &str| sgmca_feed_start_args(&feed(name));
        assert_eq!(args("depoaudio_sgmca_ab12_lead312.ffconcat"), ["-itsoffset", "-6500us"]);
        // 311 samples are 6479.17 us: the nearest microsecond, which FFmpeg
        // rounds back to exactly 311 samples.
        assert_eq!(args("depoaudio_sgmca_ab12_lead311.ffconcat"), ["-itsoffset", "-6479us"]);
        assert_eq!(
            args("depoaudio_sgmca_ab12_lead4152.ffconcat"),
            ["-itsoffset", "-86500us"]
        );
        assert!(args("depoaudio_sgmca_ab12_lead0.ffconcat").is_empty());
        // Not a feed this app wrote, or no pre-skip in its name.
        assert!(args("other_lead312.ffconcat").is_empty());
        assert!(args("depoaudio_sgmca_ab12_lead312.wav").is_empty());
        assert!(args("depoaudio_sgmca_ab12.ffconcat").is_empty());
    }

    #[test]
    fn bare_ogg_sgmca_is_used_in_place() {
        let dir = ReservationTestDir::new();
        let source = dir.path().join("plain.sgmca");
        fs::write(&source, crate::sgmca::fixtures::synthetic(5, 312, 0, |_| 30)).unwrap();

        let (prepared, guard) = prepare_audio_feed_with_cancel(&source, None).unwrap();
        assert!(guard.is_none());
        assert_eq!(prepared, source);
        assert_eq!(input_codec_args(&prepared), ["-c:a", "libopus"]);
    }

    #[test]
    fn sgmca_feed_honors_cancellation_while_reading() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let dir = ReservationTestDir::new();
        let source = dir.path().join("large-recording.sgmca");
        fs::write(
            &source,
            crate::sgmca::fixtures::synthetic_direct(20_000, 312, 0, |_| 30),
        )
        .unwrap();
        let checks = AtomicUsize::new(0);
        let cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 4;

        let error = prepare_audio_feed_with_cancel(&source, Some(&cancelled)).unwrap_err();

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

        let prepared = prepare_audio_feed_detailed(&source, None).unwrap();
        assert_ne!(prepared.path, source);
        assert!(prepared.path.starts_with(crate::workdir::tests::test_root()));
        assert_eq!(prepared.path.extension().unwrap(), "wav");
        let wav = fs::read(&prepared.path).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        let timeline = prepared.dm_timeline.clone().unwrap();
        assert_eq!(timeline.blocks_erased, 1);
        let feed = prepared.path.clone();
        drop(prepared);
        assert!(!feed.exists(), "the temporary WAV goes with its guard");
        assert_eq!(fs::read(&source).unwrap(), bytes, "the original is only read");

        let checks = AtomicUsize::new(0);
        let cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 2;
        let error = prepare_audio_feed_with_cancel(&source, Some(&cancelled)).unwrap_err();
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
        let error = prepare_audio_feed_detailed(&erased, None).err().unwrap();
        assert!(error.contains("no audio left"), "{error}");
        assert!(crate::workdir::tests::thread_run_files().is_empty());

        // A copy cut off 300 bytes into its last block still gives the
        // complete blocks before it, and the queue still gets a duration.
        let cut = dir.path().join("Session 1 (cut).dm");
        fs::write(&cut, &bytes[..bytes.len() - 220]).unwrap();
        let prepared = prepare_audio_feed_detailed(&cut, None).unwrap();
        let timeline = prepared.dm_timeline.clone().unwrap();
        assert_eq!(timeline.blocks_total, 2);
        assert_eq!(timeline.truncated_tail_bytes, 308);
        let wav = fs::read(&prepared.path).unwrap();
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
