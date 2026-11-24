//! Export manifest (DA-08): `<output>.depoaudio.json`, written next to every
//! converted file as a new file. It records what was converted (source and
//! output SHA-256), by which build, with which exact FFmpeg filter chain and
//! encoder arguments, which channel and label, and the recording's start time
//! when the source carries one. It holds file names only, never folder paths.
//!
//! The published JSON Schema is `docs/export-manifest.schema.json`; a test
//! below checks real manifests against it, so the two cannot drift.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub(crate) const MANIFEST_SUFFIX: &str = ".depoaudio.json";
const MANIFEST_KIND: &str = "depoaudio.export-manifest";
const MANIFEST_VERSION: u32 = 1;
const MAX_LABEL_CHARS: usize = 100;
const HASH_CHUNK_BYTES: usize = 1024 * 1024;
pub(crate) const HASH_CANCELLED_MESSAGE: &str = "Hashing cancelled";

/// `<output>.depoaudio.json`, in the same folder as the output.
pub(crate) fn manifest_path_for(output: &Path) -> PathBuf {
    let mut name = output.file_name().unwrap_or_default().to_os_string();
    name.push(MANIFEST_SUFFIX);
    output.with_file_name(name)
}

/// Streamed SHA-256 of a file as lowercase hex. `cancelled` is checked between
/// chunks so a long hash stops with the conversion.
pub(crate) fn sha256_file(path: &Path, cancelled: &dyn Fn() -> bool) -> Result<(String, u64), String> {
    let mut file =
        fs::File::open(path).map_err(|error| format!("Could not read the file to fingerprint it: {error}"))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    let mut total = 0u64;
    loop {
        if cancelled() {
            return Err(HASH_CANCELLED_MESSAGE.into());
        }
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("Could not read the file to fingerprint it: {error}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((hex(&hasher.finalize()), total))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

// ── Start time (BWF bext TimeReference) ─────────────────────────────────────

/// The source's start as a BWF `bext` TimeReference: samples since midnight
/// at the source sample rate. FFmpeg's WAV demuxer exposes it as the format
/// tag `time_reference` (checked with FFmpeg 6.1.1, see CHANGELOG). Only the
/// WAV demuxer reads a bext chunk (it also serves RF64 and BW64 files under
/// the same `wav` format name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceTiming {
    pub time_reference_samples: u64,
    pub sample_rate: u32,
}

/// Facts about the conversion input that the manifest needs, read from one
/// ffprobe `-show_format -show_streams -select_streams a` JSON document
/// (`crate::ffmpeg::manifest_probe_args`): every audio stream in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProbedSource {
    /// Channels of the converted stream (`SOURCE_AUDIO_STREAM`).
    pub channels: Option<u32>,
    pub timing: Option<SourceTiming>,
    /// Sample rate of the converted stream as ffprobe reports it, which is
    /// the rate leading-silence trim counts samples at.
    pub sample_rate: Option<u32>,
    /// The BWF bext origination date as `YYYY-MM-DD`, read only from a WAV
    /// that also has a bext time reference. FFmpeg 6.1 reports the bext
    /// OriginationDate as the format tag `date` (checked with a file FFmpeg
    /// wrote); a RIFF INFO creation date (ICRD) has the same tag name, so the
    /// export record calls it the file's date. With `timing` it dates the
    /// start of an SGMCA output.
    pub origination_date: Option<String>,
    /// How many audio streams the input has; None when the probe failed.
    pub audio_stream_count: Option<u32>,
    /// How far, in microseconds, the converted stream starts after the
    /// input's earliest stream (ffprobe's stream `start_time` less the
    /// format's), when that is later: an audio track that starts 0.5 s into
    /// a video, for example. FFmpeg keeps that offset in an M4A or Opus
    /// output; processing that restarts the timeline at 0 (trim, fade)
    /// puts it back with `-output_ts_offset` (`output_start_offset_args`).
    pub start_offset_micros: Option<u64>,
}

/// ffprobe's `start_time` text ("0.500000") in whole microseconds.
fn start_time_micros(value: &serde_json::Value) -> Option<i64> {
    let seconds: f64 = value.as_str()?.trim().parse().ok()?;
    seconds.is_finite().then(|| (seconds * 1_000_000.0).round() as i64)
}

/// The converted stream's start after the input's earliest stream, when it
/// is later (see `ProbedSource::start_offset_micros`).
fn stream_start_offset_micros(probe: &serde_json::Value, stream: &serde_json::Value) -> Option<u64> {
    let offset = start_time_micros(&stream["start_time"])? - start_time_micros(&probe["format"]["start_time"])?;
    u64::try_from(offset).ok().filter(|offset| *offset > 0)
}

/// A bext origination date (`yyyy-mm-dd`; EBU Tech 3285 allows any
/// separator) as `YYYY-MM-DD`, or None unless it is a real date.
fn origination_date(text: &str) -> Option<String> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4].is_ascii_digit() || bytes[7].is_ascii_digit() {
        return None;
    }
    let number = |from: usize, to: usize| -> Option<u32> {
        let part = text.get(from..to)?;
        part.bytes()
            .all(|byte| byte.is_ascii_digit())
            .then(|| part.parse().ok())?
    };
    let (year, month, day) = (number(0, 4)?, number(5, 7)?, number(8, 10)?);
    let month = time::Month::try_from(u8::try_from(month).ok()?).ok()?;
    time::Date::from_calendar_date(i32::try_from(year).ok()?, month, u8::try_from(day).ok()?).ok()?;
    Some(format!("{year:04}-{:02}-{day:02}", u8::from(month)))
}

fn tag<'a>(tags: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    tags.as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .and_then(|(_, value)| value.as_str())
}

fn positive_u32(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|count| u32::try_from(count).ok())
        .filter(|count| *count > 0)
}

fn stream_sample_rate(stream: &serde_json::Value) -> Option<u32> {
    stream["sample_rate"]
        .as_str()
        .and_then(|rate| rate.trim().parse::<u32>().ok())
        .filter(|rate| *rate > 0)
}

/// Number of audio streams in a `-select_streams a` document. An empty or
/// missing list is None: nothing was read, so nothing is claimed.
fn audio_stream_count(probe: &serde_json::Value) -> Option<u32> {
    probe["streams"]
        .as_array()
        .and_then(|streams| u32::try_from(streams.len()).ok())
        .filter(|count| *count > 0)
}

pub(crate) fn parse_probe(probe: &serde_json::Value) -> ProbedSource {
    let stream = &probe["streams"][crate::helpers::SOURCE_AUDIO_STREAM as usize];
    let channels = positive_u32(&stream["channels"]);
    let sample_rate = stream_sample_rate(stream);
    // A `time_reference` tag in any other container (FLAC, MP3, Ogg, MP4) is
    // a plain tag, not a bext chunk: FFmpeg copies the tag from a WAV input
    // into those outputs without rescaling it, so it is not a trusted start.
    let from_wav = probe["format"]["format_name"].as_str() == Some("wav");
    let time_reference = tag(&probe["format"]["tags"], "time_reference")
        .filter(|_| from_wav)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<u64>().ok());
    let origination_date = tag(&probe["format"]["tags"], "date")
        .filter(|_| from_wav && time_reference.is_some())
        .and_then(origination_date);
    let timing = match (time_reference, sample_rate) {
        (Some(time_reference_samples), Some(sample_rate)) => Some(SourceTiming {
            time_reference_samples,
            sample_rate,
        }),
        _ => None,
    };
    ProbedSource {
        channels,
        timing,
        sample_rate,
        origination_date,
        audio_stream_count: audio_stream_count(probe),
        start_offset_micros: stream_start_offset_micros(probe, stream),
    }
}

// ── Audio streams (Convert queue notice) ────────────────────────────────────

const MAX_STREAM_TAG_CHARS: usize = 80;

/// One audio stream of a queued file, as ffprobe lists it.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioStreamSummary {
    pub channels: Option<u32>,
    pub sample_rate: Option<u32>,
    /// The stream's `language` tag; FFmpeg's "und" (undetermined) is None.
    pub language: Option<String>,
    /// The stream's `title` tag (often a track or room name).
    pub title: Option<String>,
}

/// The audio streams of a queued file and which one is converted, for the
/// Convert queue's multi-track notice and the WAV size estimate.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioStreamsInfo {
    /// Zero-based index among `streams` of the stream DepoAudio converts.
    pub converted_index: u32,
    pub streams: Vec<AudioStreamSummary>,
    pub duration_secs: Option<f64>,
    /// For an SGMCA file: how many recording sessions (segments) it holds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_sessions: Option<u32>,
    /// For an SGMCA file: it looked still recording or cut short.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub still_recording: Option<bool>,
    /// For an SGMCA file: bytes after the last complete page read, which are
    /// not converted (see `SgmcaInfo::unread_bytes`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unread_bytes: Option<u64>,
    /// For an SGMCA file: the recording sessions that are not read, and why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub omitted_sessions: Option<Vec<crate::sgmca::OmittedSegment>>,
    /// For an SGMCA file: a last session shorter than its header says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut_short_session: Option<u32>,
    /// For a Liberty / BIS Digital `.dcr`: the reader's summary (codec,
    /// channel names, remote-participant changes, or why it cannot be
    /// converted). Absent for every other file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dcr: Option<crate::dcr::DcrQueueInfo>,
    /// For a DigitalCAT `.dm`: blocks skipped and whether the stamps run
    /// backwards, read from the file's own header and block headers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dm: Option<crate::dm::DmQueueInfo>,
}

/// The queue's view of an SGMCA file, from its own headers and pages (ffprobe
/// cannot read the Direct Audio layout): one audio stream, the joined
/// duration of every recording session, and how many sessions there are.
pub(crate) fn sgmca_audio_streams(info: &crate::sgmca::SgmcaInfo) -> AudioStreamsInfo {
    AudioStreamsInfo {
        converted_index: crate::helpers::SOURCE_AUDIO_STREAM,
        streams: vec![AudioStreamSummary {
            channels: Some(u32::from(info.head.channels)),
            // Opus always decodes at 48 kHz.
            sample_rate: Some(48_000),
            language: None,
            title: None,
        }],
        duration_secs: (info.duration_seconds > 0.0).then_some(info.duration_seconds),
        recording_sessions: Some(info.segments.len() as u32),
        still_recording: Some(info.growing),
        unread_bytes: Some(info.unread_bytes),
        omitted_sessions: Some(info.omitted_segments.clone()),
        cut_short_session: info.cut_short_segment,
        dcr: None,
        dm: None,
    }
}

fn bounded_tag(tags: &serde_json::Value, key: &str) -> Option<String> {
    let value: String = tag(tags, key)?
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_STREAM_TAG_CHARS)
        .collect();
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn positive_seconds(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|duration| duration.is_finite() && *duration > 0.0)
}

/// Matroska keeps a stream's length only in its `DURATION` tag, written as
/// `HH:MM:SS.fraction`.
fn clock_seconds(text: &str) -> Option<f64> {
    let mut parts = text.trim().split(':');
    let (hours, minutes, seconds) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    let seconds: f64 = seconds.parse().ok()?;
    if minutes >= 60 || !(0.0..60.0).contains(&seconds) {
        return None;
    }
    Some(f64::from(hours) * 3600.0 + f64::from(minutes) * 60.0 + seconds).filter(|total| *total > 0.0)
}

/// Duration in seconds of the converted stream (`streams[SOURCE_AUDIO_STREAM]`
/// of a `-select_streams a` or `a:0` document): its own `duration`, else its
/// Matroska `DURATION` tag, else the container's `format.duration`. The fade-out
/// and the WAV size estimate are placed on this stream, which can be shorter
/// than the container (a video longer than its first audio track, or a
/// second, longer audio track).
pub(crate) fn converted_stream_duration(probe: &serde_json::Value) -> Option<f64> {
    let stream = &probe["streams"][crate::helpers::SOURCE_AUDIO_STREAM as usize];
    stream["duration"]
        .as_str()
        .and_then(positive_seconds)
        .or_else(|| tag(&stream["tags"], "DURATION").and_then(clock_seconds))
        .or_else(|| probe["format"]["duration"].as_str().and_then(positive_seconds))
}

/// Read `crate::ffmpeg::manifest_probe_args` output for the queue notice.
pub(crate) fn parse_audio_streams(probe: &serde_json::Value) -> AudioStreamsInfo {
    let streams = probe["streams"]
        .as_array()
        .map(|streams| {
            streams
                .iter()
                .map(|stream| AudioStreamSummary {
                    channels: positive_u32(&stream["channels"]),
                    sample_rate: stream_sample_rate(stream),
                    language: bounded_tag(&stream["tags"], "language")
                        .filter(|language| !language.eq_ignore_ascii_case("und")),
                    title: bounded_tag(&stream["tags"], "title"),
                })
                .collect()
        })
        .unwrap_or_default();
    let duration_secs = converted_stream_duration(probe);
    AudioStreamsInfo {
        converted_index: crate::helpers::SOURCE_AUDIO_STREAM,
        streams,
        duration_secs,
        recording_sessions: None,
        still_recording: None,
        unread_bytes: None,
        omitted_sessions: None,
        cut_short_session: None,
        dcr: None,
        dm: None,
    }
}

/// Rescale a TimeReference to another sample rate, rounding to the nearest
/// sample. The bext field counts samples, so a 44.1 kHz source written at
/// 48 kHz needs a new count for the same wall-clock start.
pub(crate) fn rescale_time_reference(timing: SourceTiming, output_rate: u32) -> u64 {
    let scaled = (u128::from(timing.time_reference_samples) * u128::from(output_rate)
        + u128::from(timing.sample_rate) / 2)
        / u128::from(timing.sample_rate);
    u64::try_from(scaled).unwrap_or(u64::MAX)
}

/// The source start moved later by `trimmed_samples` (counted at the
/// source rate, as leading-silence trim counts them): the start of the
/// output's first sample. Saturates rather than wrap.
pub(crate) fn shifted_timing(timing: SourceTiming, trimmed_samples: u64) -> SourceTiming {
    SourceTiming {
        time_reference_samples: timing.time_reference_samples.saturating_add(trimmed_samples),
        sample_rate: timing.sample_rate,
    }
}

/// The sample rate the encoder writes, read from the exact `-ar` argument.
pub(crate) fn output_sample_rate(encoder_args: &[String]) -> Option<u32> {
    encoder_args
        .windows(2)
        .find(|pair| pair[0] == "-ar")
        .and_then(|pair| pair[1].parse().ok())
}

/// FFmpeg arguments that carry the start into a WAV output's own bext chunk
/// (`-write_bext 1` plus the rescaled `time_reference`). `trimmed_samples` is
/// what leading-silence trim removed (0 without trim), counted at the source
/// rate: the output's first sample is that much later than the source's, so
/// its start is the source start plus that amount. Empty when the output is
/// not WAV or the start is unknown.
pub(crate) fn bext_args(
    format: &str,
    timing: Option<SourceTiming>,
    output_rate: Option<u32>,
    trimmed_samples: u64,
) -> Vec<String> {
    match (format, timing, output_rate) {
        ("wav", Some(timing), Some(rate)) => vec![
            "-write_bext".into(),
            "1".into(),
            "-metadata".into(),
            format!(
                "time_reference={}",
                rescale_time_reference(shifted_timing(timing, trimmed_samples), rate)
            ),
        ],
        _ => Vec::new(),
    }
}

/// The start-time arguments for one output. A WAV output with a known start
/// gets its own bext chunk (`bext_args`). Every other output
/// gets `-metadata time_reference=`, which deletes the tag: FFmpeg would
/// otherwise copy the source's `time_reference` into FLAC, MP3 and Opus
/// outputs unrescaled, and the output would contradict its export record.
pub(crate) fn start_time_metadata_args(
    format: &str,
    timing: Option<SourceTiming>,
    output_rate: Option<u32>,
    trimmed_samples: u64,
) -> Vec<String> {
    let bext = bext_args(format, timing, output_rate, trimmed_samples);
    if bext.is_empty() {
        vec!["-metadata".into(), "time_reference=".into()]
    } else {
        bext
    }
}

// ── Manifest document ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppRecord {
    pub name: &'static str,
    pub version: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileRecord {
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
    /// For an SGMCA source: its layout and recording sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sgmca: Option<SgmcaSourceRecord>,
    /// For a Liberty / BIS Digital `.dcr` source: what the reader found
    /// (format facts and channel names only; never case, docket or password
    /// fields). Absent for every other source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dcr: Option<crate::dcr::DcrSourceRecord>,
    /// For a DigitalCAT `.dm` source: its blocks, the blocks skipped and where
    /// the time stamps jump. Absent for every other source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dm_timeline: Option<crate::dm::DmTimeline>,
}

// ── SGMCA (Direct Audio) ────────────────────────────────────────────────────

/// One recording session (segment) of an SGMCA file. The recording
/// identifier in the file header is not recorded: it identifies a recording
/// session, and the source is already identified by its SHA-256.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SgmcaSegmentRecord {
    /// The segment header's start text as stored; null for a bare Ogg file.
    pub start_text: Option<String>,
    /// Local wall-clock start `YYYY-MM-DDTHH:MM:SS`, when the text parses.
    pub start_local: Option<String>,
    /// The stored UTC offset `±HH:MM`, when the text parses.
    pub utc_offset: Option<String>,
    /// Where the session starts in the joined audio, in 48 kHz samples.
    pub start_sample: u64,
    pub duration_secs: f64,
    pub channels: u32,
    pub mapping_family: u32,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SgmcaSourceRecord {
    /// The file header's format version; null for a bare Ogg file.
    pub file_version: Option<u32>,
    pub guid_present: bool,
    pub segments: Vec<SgmcaSegmentRecord>,
    /// The file looked still recording or cut short; only complete pages
    /// were converted.
    pub growing: bool,
    /// Bytes of the file after the last complete Ogg page converted, not
    /// counting the headers of sessions left out; none of them is converted.
    pub unread_bytes: u64,
    /// Recording sessions in the file that were not converted, and why.
    pub omitted_segments: Vec<SgmcaOmittedRecord>,
    /// No Direct Audio header: a bare Ogg Opus stream (older DepoAudio).
    pub legacy_bare_ogg: bool,
}

/// A recording session of an SGMCA source that was not converted.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SgmcaOmittedRecord {
    /// 1-based position of the segment header in the file.
    pub index: u32,
    /// `zero-length-header`, `no-complete-audio` or `empty`.
    pub reason: &'static str,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SgmcaOutputRecord {
    pub file_version: u32,
    pub guid_present: bool,
    /// Where the segment start times came from: `sgmca-segment-start`,
    /// `bwf-bext` or `conversion-time`.
    pub start_source: &'static str,
    pub start_note: String,
    /// How far the output's first sample falls after the whole second its
    /// first segment header names, in milliseconds: the header holds whole
    /// seconds, and no audio is cut to make the start round. Null when the
    /// start was copied from the source's own segment headers or is the
    /// time of conversion.
    pub start_offset_ms: Option<f64>,
    /// The output's segments as read back from the written file.
    pub segments: Vec<SgmcaSegmentRecord>,
}

fn microseconds(seconds: f64) -> f64 {
    (seconds * 1_000_000.0).round() / 1_000_000.0
}

pub(crate) fn sgmca_segment_records(info: &crate::sgmca::SgmcaInfo) -> Vec<SgmcaSegmentRecord> {
    info.segments
        .iter()
        .map(|segment| SgmcaSegmentRecord {
            start_text: segment.start.as_ref().map(|start| start.text.clone()),
            start_local: segment.start.as_ref().and_then(|start| start.local.clone()),
            utc_offset: segment.start.as_ref().and_then(|start| start.utc_offset.clone()),
            start_sample: segment.start_sample,
            duration_secs: microseconds(segment.duration_seconds),
            channels: u32::from(segment.head.channels),
            mapping_family: u32::from(segment.head.mapping_family),
        })
        .collect()
}

pub(crate) fn sgmca_source_record(info: &crate::sgmca::SgmcaInfo) -> SgmcaSourceRecord {
    SgmcaSourceRecord {
        file_version: info.file_version,
        guid_present: info.guid_present,
        segments: sgmca_segment_records(info),
        growing: info.growing,
        unread_bytes: info.unread_bytes,
        omitted_segments: info
            .omitted_segments
            .iter()
            .map(|omitted| SgmcaOmittedRecord {
                index: omitted.number,
                reason: omitted.reason.as_str(),
            })
            .collect(),
        legacy_bare_ogg: info.container == crate::sgmca::Container::BareOgg,
    }
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputRecord {
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub format: String,
    pub sample_rate: Option<u32>,
    /// "riff" or "rf64", read from a WAV output's first four bytes after
    /// encoding; None for every other format.
    pub wav_header: Option<&'static str>,
    /// For an SGMCA output: its segments and where their start times came
    /// from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sgmca: Option<SgmcaOutputRecord>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessingRecord {
    pub mode: String,
    pub filter_option: Option<&'static str>,
    pub filter_chain: Option<String>,
    pub encoder_args: Vec<String>,
    /// The `-map` arguments of this output: `-map 0:a:0` (the source
    /// stream) for Mix to Stereo and Keep Original, `-map [opN]` (this
    /// channel's filter branch) for Split.
    pub map_args: Vec<String>,
    /// Present when Fade placed a fade-out; null otherwise.
    pub fade_out: Option<FadeOutRecord>,
    /// Present when Trim silence ran for this output (never in Split
    /// Channels), with `samples` 0 when nothing was below the threshold at
    /// the start; null otherwise.
    pub leading_trim: Option<LeadingTrimRecord>,
}

/// How much leading-silence trim removed, measured before the conversion.
/// The conversion removes exactly `samples` samples from the start with
/// `atrim=start_sample=<samples>`, so the output's first sample is source
/// sample `samples`.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LeadingTrimRecord {
    /// Samples removed from the start of every channel, at `sampleRate`.
    pub samples: u64,
    /// `samples` over `sampleRate`.
    pub seconds: f64,
    /// The converted stream's sample rate, which `samples` counts at.
    pub sample_rate: u32,
    /// The silence threshold the detection used, in dBFS: the output starts
    /// at the first sample above it on any channel.
    pub threshold_db: f64,
    /// The converted stream's decoded length in samples, from the same pass.
    pub decoded_source_samples: u64,
    /// The exact FFmpeg filter that measured `samples`: a sample count, the
    /// trim detection and a second sample count, on the decoded stream.
    pub measured_with: String,
}

/// Where a fade-out was placed, and the lengths it was placed from.
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FadeOutRecord {
    /// The `st` of the `afade=t=out` filter, in seconds on the output's
    /// timeline: after Trim silence, the decoded length less what trim
    /// removed, less the fade length.
    pub start_seconds: f64,
    pub duration_seconds: f64,
    /// The converted stream's length as FFmpeg decodes it, which places the
    /// fade-out.
    pub decoded_source_seconds: f64,
    /// The length the source's headers state, for comparison; None when
    /// they state none.
    pub stated_source_seconds: Option<f64>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChannelRecord {
    pub output: &'static str,
    pub source_channel_count: Option<u32>,
    pub source_channel_index: Option<u32>,
    pub label: Option<String>,
    pub labels: Vec<String>,
}

/// Which audio stream of the source was converted. Every probe and FFmpeg
/// pass maps this one stream explicitly (`-map 0:a:<index>`).
#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AudioStreamRecord {
    pub index: u32,
    pub source_audio_stream_count: Option<u32>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StartTimeRecord {
    pub known: bool,
    pub source: Option<&'static str>,
    pub source_time_reference_samples: Option<u64>,
    pub source_sample_rate: Option<u32>,
    pub seconds_since_midnight: Option<f64>,
    pub output_time_reference_samples: Option<u64>,
    pub written_to_output: bool,
    pub note: String,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportManifest {
    pub kind: &'static str,
    pub manifest_version: u32,
    pub created_at: String,
    pub app: AppRecord,
    pub source: FileRecord,
    pub output: OutputRecord,
    pub processing: ProcessingRecord,
    pub channel: ChannelRecord,
    pub audio_stream: AudioStreamRecord,
    pub start_time: StartTimeRecord,
}

/// Everything about one output that the conversion knows once it has run.
pub(crate) struct OutputFacts<'a> {
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub format: &'a str,
    pub mode: &'a str,
    pub filter_option: Option<&'static str>,
    pub filter_chain: Option<String>,
    /// Encoder plus metadata arguments for this output, without its path.
    pub encoder_args: Vec<String>,
    /// This output's `-map` arguments.
    pub map_args: Vec<String>,
    pub fade_out: Option<FadeOutRecord>,
    pub source_channel_index: Option<u32>,
    pub label: Option<String>,
    /// From `wav_header_kind`, for WAV outputs only.
    pub wav_header: Option<&'static str>,
    /// What leading-silence trim removed from this output, if anything.
    pub leading_trim: Option<LeadingTrimRecord>,
    /// For SGMCA outputs only.
    pub sgmca: Option<SgmcaOutputRecord>,
}

/// The header a finished WAV output was written with, from its first four
/// bytes: `RIFF` is "riff", `RF64` is "rf64". Anything else is an error, so
/// the record never names a header that is not in the file.
pub(crate) fn wav_header_kind(path: &Path) -> Result<&'static str, String> {
    let mut magic = [0u8; 4];
    fs::File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .map_err(|error| format!("Could not read the WAV output's header: {error}"))?;
    match &magic {
        b"RIFF" => Ok("riff"),
        b"RF64" => Ok("rf64"),
        _ => Err("The WAV output does not start with a RIFF or RF64 header.".into()),
    }
}

fn bounded_label(label: &str) -> String {
    label.trim().chars().take(MAX_LABEL_CHARS).collect()
}

pub(crate) fn start_time_record(
    timing: Option<SourceTiming>,
    output_rate: Option<u32>,
    leading_trim: Option<&LeadingTrimRecord>,
    format: &str,
) -> StartTimeRecord {
    let Some(timing) = timing else {
        return StartTimeRecord {
            known: false,
            source: None,
            source_time_reference_samples: None,
            source_sample_rate: None,
            seconds_since_midnight: None,
            output_time_reference_samples: None,
            written_to_output: false,
            note: "The source does not carry a BWF bext time reference, so its start time is not known.".into(),
        };
    };
    let seconds = timing.time_reference_samples as f64 / f64::from(timing.sample_rate);
    let trimmed = leading_trim.map_or(0, |trim| trim.samples);
    let output_reference = output_rate.map(|rate| rescale_time_reference(shifted_timing(timing, trimmed), rate));
    let written = !bext_args(format, Some(timing), output_rate, trimmed).is_empty();
    let note = match (trimmed > 0, written) {
        (true, true) => "Leading-silence trim removed processing.leadingTrim.samples samples from the start, so the output starts that much later than the source. The output WAV carries the source start plus that amount in its own bext chunk, rescaled to the output sample rate.",
        (true, false) => "Leading-silence trim removed processing.leadingTrim.samples samples from the start, so the output starts that much later than the source; outputTimeReferenceSamples is the source start plus that amount at the output rate. This format has no bext chunk, and DepoAudio removes the copied time_reference tag from it, so the start is recorded here only.",
        (false, true) => "The output WAV carries the same start in its own bext chunk, rescaled to the output sample rate.",
        (false, false) => "The output starts at the source start. This format has no bext chunk, and DepoAudio removes the copied time_reference tag from it, so the start is recorded here only.",
    };
    StartTimeRecord {
        known: true,
        source: Some("bwf-bext-time-reference"),
        source_time_reference_samples: Some(timing.time_reference_samples),
        source_sample_rate: Some(timing.sample_rate),
        seconds_since_midnight: Some((seconds * 1000.0).round() / 1000.0),
        output_time_reference_samples: output_reference,
        written_to_output: written,
        note: note.into(),
    }
}

pub(crate) struct ManifestInput<'a> {
    pub app_version: String,
    pub created_at: String,
    pub source: FileRecord,
    pub probed: &'a ProbedSource,
    pub labels: &'a [String],
}

pub(crate) fn build_manifest(input: &ManifestInput<'_>, facts: OutputFacts<'_>) -> ExportManifest {
    let sample_rate = output_sample_rate(&facts.encoder_args);
    let channel_count = input.probed.channels;
    let labels: Vec<String> = match channel_count {
        Some(count) => (0..count as usize)
            .map(|index| {
                input
                    .labels
                    .get(index)
                    .map(|label| bounded_label(label))
                    .unwrap_or_default()
            })
            .collect(),
        None => input.labels.iter().map(|label| bounded_label(label)).collect(),
    };
    let output_kind = match facts.mode {
        "split" => "single-channel",
        "keep" => "all-channels",
        _ => "stereo-mix",
    };
    ExportManifest {
        kind: MANIFEST_KIND,
        manifest_version: MANIFEST_VERSION,
        created_at: input.created_at.clone(),
        app: AppRecord {
            name: "DepoAudio",
            version: input.app_version.clone(),
        },
        source: input.source.clone(),
        output: OutputRecord {
            file_name: facts.file_name,
            sha256: facts.sha256,
            size_bytes: facts.size_bytes,
            format: facts.format.into(),
            sample_rate,
            wav_header: facts.wav_header,
            sgmca: facts.sgmca,
        },
        processing: ProcessingRecord {
            mode: facts.mode.into(),
            filter_option: facts.filter_option,
            filter_chain: facts.filter_chain,
            encoder_args: facts.encoder_args,
            map_args: facts.map_args,
            fade_out: facts.fade_out,
            leading_trim: facts.leading_trim.clone(),
        },
        channel: ChannelRecord {
            output: output_kind,
            source_channel_count: channel_count,
            source_channel_index: facts.source_channel_index,
            label: facts
                .label
                .map(|label| bounded_label(&label))
                .filter(|label| !label.is_empty()),
            labels,
        },
        audio_stream: AudioStreamRecord {
            index: crate::helpers::SOURCE_AUDIO_STREAM,
            source_audio_stream_count: input.probed.audio_stream_count,
        },
        start_time: start_time_record(
            input.probed.timing,
            sample_rate,
            facts.leading_trim.as_ref(),
            facts.format,
        ),
    }
}

/// Reserve an output path and its manifest path together, both as new empty
/// files (`create_new`). If either name is taken, the next numbered name is
/// tried, so a manifest never overwrites an existing file and always sits at
/// exactly `<output>.depoaudio.json`.
pub(crate) fn reserve_output_and_manifest(path: &Path) -> Result<(PathBuf, PathBuf), String> {
    crate::helpers::reserve_unique_path_with(path, |candidate| {
        let manifest = manifest_path_for(candidate);
        match fs::OpenOptions::new().write(true).create_new(true).open(&manifest) {
            Ok(_) => Ok(Some(manifest)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(None),
            Err(error) => Err(format!("Could not reserve the export record file: {error}")),
        }
    })
}

/// Write the manifest into its reserved, still-empty placeholder. The file is
/// opened without `create`, so this can only fill the file this conversion
/// reserved; it fails rather than create or replace anything else.
pub(crate) fn write_manifest(path: &Path, manifest: &ExportManifest) -> Result<(), String> {
    let mut json =
        serde_json::to_vec_pretty(manifest).map_err(|error| format!("Could not encode the export record: {error}"))?;
    json.push(b'\n');
    let existing = fs::metadata(path).map_err(|error| format!("The export record placeholder is missing: {error}"))?;
    if existing.len() != 0 || !existing.is_file() {
        return Err("The export record placeholder was changed by another program; it was left untouched.".into());
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|error| format!("Could not write the export record: {error}"))?;
    file.write_all(&json)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Could not write the export record: {error}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("depoaudio_manifest_test_{}", uuid::Uuid::new_v4().simple()));
            fs::create_dir(&path).expect("create manifest test directory");
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn probe(tags: serde_json::Value, rate: &str, channels: u64) -> serde_json::Value {
        serde_json::json!({
            "streams": [{ "sample_rate": rate, "channels": channels }],
            "format": { "format_name": "wav", "tags": tags }
        })
    }

    /// One second of leading silence at 44.1 kHz, as a measured trim.
    fn sample_trim() -> LeadingTrimRecord {
        LeadingTrimRecord {
            samples: 44_100,
            seconds: 1.0,
            sample_rate: 44_100,
            threshold_db: -50.0,
            decoded_source_samples: 441_000,
            measured_with: crate::ffmpeg::leading_trim_measure_filter(-50.0),
        }
    }

    fn sample_input(probed: &ProbedSource, labels: &[String], trimmed: bool) -> ExportManifest {
        let input = ManifestInput {
            app_version: "1.0.3".into(),
            created_at: "2026-11-13T09:00:00Z".into(),
            source: FileRecord {
                file_name: "synthetic_session.wav".into(),
                sha256: "a".repeat(64),
                size_bytes: 1024,
                sgmca: None,
                dcr: None,
                dm_timeline: None,
            },
            probed,
            labels,
        };
        build_manifest(
            &input,
            OutputFacts {
                file_name: "synthetic_session_Witness.wav".into(),
                sha256: "b".repeat(64),
                size_bytes: 512,
                format: "wav",
                mode: "split",
                filter_option: Some("-filter_complex"),
                filter_chain: Some(
                    "[0:a:0]asplit=2[sp0][sp1];[sp0]pan=mono|c0=c0[op0];[sp1]pan=mono|c0=c1[op1]".into(),
                ),
                encoder_args: vec!["-acodec".into(), "pcm_s16le".into(), "-ar".into(), "48000".into()],
                map_args: vec!["-map".into(), "[op1]".into()],
                fade_out: None,
                source_channel_index: Some(1),
                label: Some("Witness".into()),
                wav_header: Some("riff"),
                leading_trim: trimmed.then(sample_trim),
                sgmca: None,
            },
        )
    }

    fn sgmca_segment(start: &str, start_sample: u64, duration_secs: f64) -> SgmcaSegmentRecord {
        let parsed = crate::sgmca::SegmentStart::from_text(start.into());
        SgmcaSegmentRecord {
            start_text: Some(start.into()),
            start_local: parsed.local,
            utc_offset: parsed.utc_offset,
            start_sample,
            duration_secs,
            channels: 4,
            mapping_family: 255,
        }
    }

    #[test]
    fn sgmca_source_and_output_records_match_the_schema() {
        let mut manifest = sample_input(&ProbedSource::default(), &[], false);
        manifest.source.sgmca = Some(SgmcaSourceRecord {
            file_version: Some(1),
            guid_present: true,
            segments: vec![
                sgmca_segment("2026-11-22T10:15:30-0500", 0, 1.5),
                sgmca_segment("2026-11-22T11:02:05-0500", 72_000, 2.25),
                SgmcaSegmentRecord {
                    mapping_family: 1,
                    ..sgmca_segment("not a time", 180_000, 0.5)
                },
            ],
            growing: true,
            unread_bytes: 12_345,
            omitted_segments: vec![SgmcaOmittedRecord {
                index: 4,
                reason: "zero-length-header",
            }],
            legacy_bare_ogg: false,
        });
        manifest.output.format = "sgmca".into();
        manifest.output.wav_header = None;
        manifest.output.sgmca = Some(SgmcaOutputRecord {
            file_version: 1,
            guid_present: true,
            start_source: "sgmca-segment-start",
            start_note: "kept".into(),
            start_offset_ms: Some(420.0),
            segments: vec![sgmca_segment("2026-11-22T10:15:30-0500", 0, 1.5)],
        });
        let errors = schema_errors(&manifest);
        assert!(errors.is_empty(), "{errors:#?}");
        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(
            json["source"]["sgmca"]["segments"][1]["startLocal"],
            "2026-11-22T11:02:05"
        );
        assert_eq!(json["source"]["sgmca"]["segments"][1]["utcOffset"], "-05:00");
        assert_eq!(
            json["source"]["sgmca"]["segments"][2]["startLocal"],
            serde_json::Value::Null
        );
        assert!(
            json["source"]["sgmca"].get("guid").is_none(),
            "the identifier itself is never recorded"
        );

        // A bare Ogg source has no header, so no version and no start texts.
        manifest.source.sgmca = Some(SgmcaSourceRecord {
            file_version: None,
            guid_present: false,
            segments: vec![SgmcaSegmentRecord {
                start_text: None,
                start_local: None,
                utc_offset: None,
                ..sgmca_segment("", 0, 1.0)
            }],
            growing: false,
            unread_bytes: 0,
            omitted_segments: Vec::new(),
            legacy_bare_ogg: true,
        });
        assert!(schema_errors(&manifest).is_empty());

        // Records without the objects (every non-SGMCA conversion) omit them.
        let plain = serde_json::to_value(sample_input(&ProbedSource::default(), &[], false)).unwrap();
        assert!(plain["source"].get("sgmca").is_none());
        assert!(plain["output"].get("sgmca").is_none());

        assert_eq!(json["source"]["sgmca"]["unreadBytes"], 12_345);
        assert_eq!(
            json["source"]["sgmca"]["omittedSegments"],
            serde_json::json!([{ "index": 4, "reason": "zero-length-header" }])
        );

        let mut bad = json.clone();
        bad["output"]["sgmca"]["startSource"] = "guessed".into();
        assert!(!schema_errors_for_value(&bad).is_empty());
        let mut bad = json.clone();
        bad["source"]["sgmca"]["omittedSegments"][0]["reason"] = "skipped".into();
        assert!(!schema_errors_for_value(&bad).is_empty());
        let mut bad = json.clone();
        bad["source"]["sgmca"].as_object_mut().unwrap().remove("unreadBytes");
        assert!(!schema_errors_for_value(&bad).is_empty());
    }

    #[test]
    fn origination_date_is_read_only_as_a_real_date_from_a_bwf() {
        let probed = parse_probe(&probe(
            serde_json::json!({ "time_reference": "48000", "date": "2026:11:22" }),
            "48000",
            1,
        ));
        assert_eq!(probed.origination_date.as_deref(), Some("2026-11-22"));
        for bad in ["2026-13-01", "2026-02-30", "20261122", "2026-1-22", ""] {
            let probed = parse_probe(&probe(
                serde_json::json!({ "time_reference": "48000", "date": bad }),
                "48000",
                1,
            ));
            assert_eq!(probed.origination_date, None, "{bad}");
        }
        // Without a bext time reference a date tag is not used.
        let undated = parse_probe(&probe(serde_json::json!({ "date": "2026-11-22" }), "48000", 1));
        assert_eq!(undated.origination_date, None);
        let mut flac = probe(
            serde_json::json!({ "time_reference": "48000", "date": "2026-11-22" }),
            "48000",
            1,
        );
        flac["format"]["format_name"] = "flac".into();
        assert_eq!(parse_probe(&flac).origination_date, None);
    }

    #[test]
    fn manifest_path_appends_the_suffix_to_the_full_output_name() {
        assert_eq!(
            manifest_path_for(Path::new("/out/session_Witness.wav")),
            PathBuf::from("/out/session_Witness.wav.depoaudio.json")
        );
    }

    #[test]
    fn sha256_matches_the_known_digest_and_stops_on_cancel() {
        let dir = TestDir::new();
        let path = dir.0.join("abc.bin");
        fs::write(&path, b"abc").unwrap();
        let (digest, size) = sha256_file(&path, &|| false).unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(size, 3);
        assert_eq!(sha256_file(&path, &|| true).unwrap_err(), HASH_CANCELLED_MESSAGE);
    }

    #[test]
    fn probe_reads_bext_time_reference_and_rate() {
        let probed = parse_probe(&probe(serde_json::json!({ "time_reference": "158760000" }), "44100", 2));
        assert_eq!(probed.channels, Some(2));
        assert_eq!(
            probed.timing,
            Some(SourceTiming {
                time_reference_samples: 158_760_000,
                sample_rate: 44_100
            })
        );
        // Upper-case tag names (some ffprobe builds) are read the same way.
        let upper = parse_probe(&probe(serde_json::json!({ "TIME_REFERENCE": "48000" }), "48000", 1));
        assert_eq!(upper.timing.unwrap().time_reference_samples, 48_000);
    }

    #[test]
    fn probe_without_a_usable_time_reference_reports_no_start() {
        for tags in [
            serde_json::json!({}),
            serde_json::json!({ "time_reference": "" }),
            serde_json::json!({ "time_reference": "-5" }),
            serde_json::json!({ "time_reference": "12abc" }),
            serde_json::Value::Null,
        ] {
            assert_eq!(parse_probe(&probe(tags, "48000", 1)).timing, None);
        }
        // A time reference without a sample rate cannot be converted to time.
        let no_rate = serde_json::json!({ "streams": [{}], "format": { "tags": { "time_reference": "10" } } });
        assert_eq!(parse_probe(&no_rate).timing, None);
    }

    #[test]
    fn probe_reads_time_reference_only_from_a_wav_container() {
        // FLAC, MP3 and Ogg have no bext chunk. A plain `time_reference` tag
        // there (for example one copied by an older conversion, not
        // rescaled) is not a BWF start and must not be trusted.
        for format_name in ["flac", "mp3", "ogg", "mov,mp4,m4a,3gp,3g2,mj2", "w64", ""] {
            let tagged = serde_json::json!({
                "streams": [{ "sample_rate": "48000", "channels": 1 }],
                "format": { "format_name": format_name, "tags": { "time_reference": "158760000" } }
            });
            assert_eq!(parse_probe(&tagged).timing, None, "{format_name}");
        }
        let unnamed = serde_json::json!({
            "streams": [{ "sample_rate": "48000", "channels": 1 }],
            "format": { "tags": { "time_reference": "158760000" } }
        });
        assert_eq!(parse_probe(&unnamed).timing, None);
        let wav = serde_json::json!({
            "streams": [{ "sample_rate": "48000", "channels": 1 }],
            "format": { "format_name": "wav", "tags": { "time_reference": "158760000" } }
        });
        assert!(parse_probe(&wav).timing.is_some());
    }

    #[test]
    fn every_output_without_a_bext_chunk_drops_the_copied_time_reference() {
        let timing = Some(SourceTiming {
            time_reference_samples: 48_000,
            sample_rate: 48_000,
        });
        let clear = vec!["-metadata".to_string(), "time_reference=".to_string()];
        assert_eq!(
            start_time_metadata_args("wav", timing, Some(16_000), 0),
            bext_args("wav", timing, Some(16_000), 0)
        );
        for format in ["flac", "mp3", "opus", "m4a"] {
            assert_eq!(
                start_time_metadata_args(format, timing, Some(48_000), 0),
                clear,
                "{format}"
            );
            assert_eq!(
                start_time_metadata_args(format, timing, Some(48_000), 4_800),
                clear,
                "{format}"
            );
            assert_eq!(
                start_time_metadata_args(format, None, Some(48_000), 0),
                clear,
                "{format}"
            );
        }
        assert_eq!(
            start_time_metadata_args("wav", timing, Some(48_000), 4_800),
            bext_args("wav", timing, Some(48_000), 4_800)
        );
        assert_eq!(start_time_metadata_args("wav", None, Some(48_000), 0), clear);
        assert_eq!(start_time_metadata_args("wav", timing, None, 0), clear);
    }

    #[test]
    fn time_reference_is_rescaled_to_the_output_rate() {
        // 01:00:00 at 44.1 kHz is 158,760,000 samples; at 48 kHz, 172,800,000.
        let timing = SourceTiming {
            time_reference_samples: 158_760_000,
            sample_rate: 44_100,
        };
        assert_eq!(rescale_time_reference(timing, 48_000), 172_800_000);
        assert_eq!(rescale_time_reference(timing, 44_100), 158_760_000);
        // Rounds to the nearest sample: 1 sample at 44.1 kHz is 1.088 at 48 kHz.
        let one = SourceTiming {
            time_reference_samples: 1,
            sample_rate: 44_100,
        };
        assert_eq!(rescale_time_reference(one, 48_000), 1);
        let huge = SourceTiming {
            time_reference_samples: u64::MAX,
            sample_rate: 8_000,
        };
        assert_eq!(rescale_time_reference(huge, 192_000), u64::MAX);
    }

    #[test]
    fn bext_is_written_for_wav_with_a_known_start_moved_by_the_trim() {
        let timing = Some(SourceTiming {
            time_reference_samples: 48_000,
            sample_rate: 48_000,
        });
        assert_eq!(
            bext_args("wav", timing, Some(16_000), 0),
            vec!["-write_bext", "1", "-metadata", "time_reference=16000"]
        );
        // 0.1 s trimmed at 48 kHz: the output starts 1.1 s after midnight.
        assert_eq!(
            bext_args("wav", timing, Some(16_000), 4_800),
            vec!["-write_bext", "1", "-metadata", "time_reference=17600"]
        );
        assert_eq!(
            bext_args("wav", timing, Some(48_000), 4_800),
            vec!["-write_bext", "1", "-metadata", "time_reference=52800"]
        );
        assert!(bext_args("wav", None, Some(16_000), 0).is_empty());
        assert!(bext_args("flac", timing, Some(16_000), 0).is_empty());
        assert!(bext_args("wav", timing, None, 0).is_empty());
    }

    #[test]
    fn a_trimmed_start_saturates_instead_of_wrapping() {
        let timing = SourceTiming {
            time_reference_samples: u64::MAX - 1,
            sample_rate: 48_000,
        };
        assert_eq!(shifted_timing(timing, 10).time_reference_samples, u64::MAX);
    }

    #[test]
    fn output_rate_comes_from_the_exact_encoder_arguments() {
        let opus = crate::helpers::output_args("opus", "16000", 192);
        assert_eq!(output_sample_rate(&opus), Some(48_000));
        let wav = crate::helpers::output_args("wav", "16000", 192);
        assert_eq!(output_sample_rate(&wav), Some(16_000));
        assert_eq!(output_sample_rate(&[]), None);
    }

    #[test]
    fn manifest_records_channel_label_hashes_and_start() {
        let probed = parse_probe(&probe(serde_json::json!({ "time_reference": "158760000" }), "44100", 2));
        let labels = vec!["Attorney".to_string(), "  Witness  ".to_string()];
        let manifest = sample_input(&probed, &labels, false);
        assert_eq!(manifest.kind, "depoaudio.export-manifest");
        assert_eq!(manifest.app.version, "1.0.3");
        assert_eq!(manifest.source.sha256, "a".repeat(64));
        assert_eq!(manifest.output.sha256, "b".repeat(64));
        assert_eq!(manifest.output.sample_rate, Some(48_000));
        assert_eq!(manifest.channel.output, "single-channel");
        assert_eq!(manifest.channel.source_channel_index, Some(1));
        assert_eq!(manifest.channel.label.as_deref(), Some("Witness"));
        assert_eq!(manifest.channel.labels, vec!["Attorney", "Witness"]);
        assert!(manifest.start_time.known);
        assert_eq!(manifest.start_time.seconds_since_midnight, Some(3600.0));
        assert_eq!(manifest.start_time.output_time_reference_samples, Some(172_800_000));
        assert!(manifest.start_time.written_to_output);

        // One second trimmed at 44.1 kHz: the output starts at 01:00:01,
        // 172,848,000 samples at 48 kHz, and its record says how much moved.
        let trimmed = sample_input(&probed, &labels, true);
        assert!(trimmed.start_time.known);
        assert_eq!(trimmed.start_time.source_time_reference_samples, Some(158_760_000));
        assert_eq!(trimmed.start_time.output_time_reference_samples, Some(172_848_000));
        assert!(trimmed.start_time.written_to_output);
        assert!(trimmed.start_time.note.contains("processing.leadingTrim.samples"));
        assert_eq!(trimmed.processing.leading_trim, Some(sample_trim()));
        let value = serde_json::to_value(&trimmed).unwrap();
        assert_eq!(value["processing"]["leadingTrim"]["samples"], 44_100);
        assert_eq!(value["processing"]["leadingTrim"]["seconds"], 1.0);
        assert_eq!(value["processing"]["leadingTrim"]["sampleRate"], 44_100);
        assert!(serde_json::to_value(&manifest).unwrap()["processing"]["leadingTrim"].is_null());

        let unknown = sample_input(&ProbedSource::default(), &labels, false);
        assert!(!unknown.start_time.known);
        assert_eq!(unknown.channel.labels, vec!["Attorney", "Witness"]);
    }

    #[test]
    fn manifest_never_contains_a_folder_path() {
        let probed = ProbedSource::default();
        let manifest = sample_input(&probed, &[], false);
        let json = serde_json::to_string(&manifest).unwrap();
        // The only slashes allowed are none at all: names, hashes and filters
        // in this fixture carry no separators, and no field holds a folder.
        assert!(!json.contains('/') && !json.contains('\\'), "{json}");
    }

    #[test]
    fn reservation_skips_names_whose_manifest_already_exists() {
        let dir = TestDir::new();
        let wanted = dir.0.join("session.wav");
        // A manifest left behind from an earlier export keeps its name.
        fs::write(manifest_path_for(&wanted), b"earlier record").unwrap();
        let (output, manifest) = reserve_output_and_manifest(&wanted).unwrap();
        assert_eq!(output, dir.0.join("session_1.wav"));
        assert_eq!(manifest, dir.0.join("session_1.wav.depoaudio.json"));
        assert!(!wanted.exists(), "the skipped output placeholder is released");
        assert_eq!(fs::read(manifest_path_for(&wanted)).unwrap(), b"earlier record");
        assert_eq!(fs::metadata(&manifest).unwrap().len(), 0);
    }

    #[test]
    fn write_fills_only_an_empty_reserved_placeholder() {
        let dir = TestDir::new();
        let probed = ProbedSource::default();
        let manifest = sample_input(&probed, &[], false);

        let missing = dir.0.join("missing.wav.depoaudio.json");
        assert!(write_manifest(&missing, &manifest).is_err());
        assert!(!missing.exists(), "writing never creates a file");

        let changed = dir.0.join("changed.wav.depoaudio.json");
        fs::write(&changed, b"someone else's data").unwrap();
        assert!(write_manifest(&changed, &manifest).is_err());
        assert_eq!(fs::read(&changed).unwrap(), b"someone else's data");

        let reserved = dir.0.join("reserved.wav.depoaudio.json");
        fs::File::create(&reserved).unwrap();
        write_manifest(&reserved, &manifest).unwrap();
        let written: serde_json::Value = serde_json::from_slice(&fs::read(&reserved).unwrap()).unwrap();
        assert_eq!(written["kind"], "depoaudio.export-manifest");
    }

    // ── Schema conformance ──────────────────────────────────────────────────
    // A small validator for the JSON Schema keywords the published schema
    // uses (type, const, enum, required, properties, additionalProperties,
    // items, minimum, maximum, pattern, if/then/else). It fails on any other keyword so the
    // schema cannot grow a rule this test silently ignores.

    const SCHEMA: &str = include_str!("../../docs/export-manifest.schema.json");

    fn type_matches(kind: &str, value: &serde_json::Value) -> bool {
        match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_u64() || value.is_i64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            other => panic!("schema uses an unsupported type {other}"),
        }
    }

    fn validate(schema: &serde_json::Value, value: &serde_json::Value, at: &str, errors: &mut Vec<String>) {
        let rules = schema.as_object().expect("schema node is an object");
        for (keyword, rule) in rules {
            match keyword.as_str() {
                "$schema" | "$id" | "title" | "description" | "then" | "else" => {}
                "if" => {
                    let mut condition = Vec::new();
                    validate(rule, value, at, &mut condition);
                    let branch = if condition.is_empty() { "then" } else { "else" };
                    if let Some(next) = rules.get(branch) {
                        validate(next, value, at, errors);
                    }
                }
                "type" => {
                    let ok = match rule {
                        serde_json::Value::String(kind) => type_matches(kind, value),
                        serde_json::Value::Array(kinds) => {
                            kinds.iter().any(|kind| type_matches(kind.as_str().unwrap(), value))
                        }
                        _ => panic!("bad type rule at {at}"),
                    };
                    if !ok {
                        errors.push(format!("{at}: expected type {rule}, got {value}"));
                    }
                }
                "const" => {
                    if rule != value {
                        errors.push(format!("{at}: expected {rule}, got {value}"));
                    }
                }
                "enum" => {
                    if !rule.as_array().unwrap().contains(value) {
                        errors.push(format!("{at}: {value} is not one of {rule}"));
                    }
                }
                "required" => {
                    // As in JSON Schema, `required` constrains objects only,
                    // so a nullable object may be null.
                    for name in rule.as_array().unwrap() {
                        let name = name.as_str().unwrap();
                        if value.is_object() && value.get(name).is_none() {
                            errors.push(format!("{at}: missing required {name}"));
                        }
                    }
                }
                "properties" => {
                    if let Some(object) = value.as_object() {
                        for (name, child) in rule.as_object().unwrap() {
                            if let Some(child_value) = object.get(name) {
                                validate(child, child_value, &format!("{at}.{name}"), errors);
                            }
                        }
                    }
                }
                "additionalProperties" => {
                    assert_eq!(
                        rule,
                        &serde_json::Value::Bool(false),
                        "only additionalProperties: false is supported"
                    );
                    let known = rules["properties"].as_object().unwrap();
                    if let Some(object) = value.as_object() {
                        for name in object.keys() {
                            if !known.contains_key(name) {
                                errors.push(format!("{at}: unexpected property {name}"));
                            }
                        }
                    }
                }
                "items" => {
                    if let Some(items) = value.as_array() {
                        for (index, item) in items.iter().enumerate() {
                            validate(rule, item, &format!("{at}[{index}]"), errors);
                        }
                    }
                }
                "minimum" | "maximum" => {
                    if let (Some(limit), Some(number)) = (rule.as_f64(), value.as_f64()) {
                        let ok = if keyword == "minimum" {
                            number >= limit
                        } else {
                            number <= limit
                        };
                        if !ok {
                            errors.push(format!("{at}: {number} breaks {keyword} {limit}"));
                        }
                    }
                }
                "pattern" => {
                    if let Some(text) = value.as_str() {
                        let regex = regex::Regex::new(rule.as_str().unwrap()).unwrap();
                        if !regex.is_match(text) {
                            errors.push(format!("{at}: {text:?} does not match {rule}"));
                        }
                    }
                }
                other => panic!("schema uses unsupported keyword {other} at {at}"),
            }
        }
    }

    fn schema_errors(manifest: &ExportManifest) -> Vec<String> {
        schema_errors_for_value(&serde_json::to_value(manifest).unwrap())
    }

    /// Schema errors for a record as written to disk (conversion tests read
    /// real `<output>.depoaudio.json` files back through this).
    pub(crate) fn schema_errors_for_value(value: &serde_json::Value) -> Vec<String> {
        let schema: serde_json::Value = serde_json::from_str(SCHEMA).expect("schema is JSON");
        let mut errors = Vec::new();
        validate(&schema, value, "$", &mut errors);
        errors
    }

    #[test]
    fn manifests_match_the_published_schema() {
        let timed = parse_probe(&probe(serde_json::json!({ "time_reference": "158760000" }), "44100", 2));
        let labels = vec!["Attorney".to_string(), "Witness".to_string()];
        for manifest in [
            sample_input(&timed, &labels, false),
            sample_input(&timed, &labels, true),
            sample_input(&ProbedSource::default(), &[], false),
        ] {
            let errors = schema_errors(&manifest);
            assert!(errors.is_empty(), "{errors:#?}");
        }
        let mut faded = sample_input(&ProbedSource::default(), &[], false);
        for stated in [Some(5.0065), None] {
            faded.processing.fade_out = Some(FadeOutRecord {
                start_seconds: 14.5,
                duration_seconds: 0.5,
                decoded_source_seconds: 15.0,
                stated_source_seconds: stated,
            });
            let errors = schema_errors(&faded);
            assert!(errors.is_empty(), "{errors:#?}");
            let value = serde_json::to_value(&faded).unwrap();
            assert_eq!(value["processing"]["fadeOut"]["decodedSourceSeconds"], 15.0);
        }
    }

    #[test]
    fn probe_describes_stream_zero_and_counts_every_audio_stream() {
        let two = serde_json::json!({
            "streams": [
                { "sample_rate": "48000", "channels": 1, "tags": { "language": "eng", "title": "Floor" } },
                { "sample_rate": "44100", "channels": 4, "tags": { "language": "und" } }
            ],
            "format": { "format_name": "matroska,webm", "duration": "2.000000" }
        });
        let probed = parse_probe(&two);
        assert_eq!(
            probed.channels,
            Some(1),
            "channels come from stream 0, not the widest stream"
        );
        assert_eq!(probed.audio_stream_count, Some(2));

        let streams = parse_audio_streams(&two);
        assert_eq!(streams.converted_index, 0);
        assert_eq!(streams.duration_secs, Some(2.0));
        assert_eq!(
            streams.streams,
            vec![
                AudioStreamSummary {
                    channels: Some(1),
                    sample_rate: Some(48_000),
                    language: Some("eng".into()),
                    title: Some("Floor".into()),
                },
                AudioStreamSummary {
                    channels: Some(4),
                    sample_rate: Some(44_100),
                    language: None,
                    title: None,
                },
            ]
        );

        // Nothing read, nothing claimed.
        assert_eq!(parse_probe(&serde_json::json!({})).audio_stream_count, None);
        assert_eq!(parse_probe(&serde_json::json!({})).start_offset_micros, None);
        assert_eq!(
            parse_probe(&serde_json::json!({ "streams": [] })).audio_stream_count,
            None
        );
        let empty = parse_audio_streams(&serde_json::json!({}));
        assert!(empty.streams.is_empty());
        assert_eq!(empty.duration_secs, None);
    }

    #[test]
    fn duration_is_the_converted_streams_not_the_containers() {
        // An MKV whose a:0 runs 4 s and a:1 10 s: format.duration is 10 s.
        let mkv = serde_json::json!({
            "streams": [
                { "channels": 1, "tags": { "DURATION": "00:00:04.000000000" } },
                { "channels": 1, "tags": { "DURATION": "00:00:10.000000000" } }
            ],
            "format": { "duration": "10.000000" }
        });
        assert_eq!(converted_stream_duration(&mkv), Some(4.0));
        assert_eq!(parse_audio_streams(&mkv).duration_secs, Some(4.0));
        // A transport stream whose video outlasts its audio.
        let ts = serde_json::json!({
            "streams": [{ "duration": "3.996733" }],
            "format": { "duration": "10.004900" }
        });
        assert_eq!(converted_stream_duration(&ts), Some(3.996733));
        assert_eq!(
            converted_stream_duration(&serde_json::json!({
                "streams": [{ "tags": { "DURATION": "01:02:03.500" } }]
            })),
            Some(3723.5)
        );
        // No stream duration: the container's, as before.
        let only_format = serde_json::json!({ "streams": [{}], "format": { "duration": "2.5" } });
        assert_eq!(converted_stream_duration(&only_format), Some(2.5));
        for broken in ["N/A", "0", "-1", "00:61:00", "1:2", "aa:bb:cc"] {
            let probe = serde_json::json!({
                "streams": [{ "duration": broken, "tags": { "DURATION": broken } }],
                "format": { "duration": "7" }
            });
            assert_eq!(converted_stream_duration(&probe), Some(7.0), "{broken}");
        }
        assert_eq!(converted_stream_duration(&serde_json::json!({})), None);
    }

    #[test]
    fn stream_tags_are_bounded_and_stripped_of_control_characters() {
        let probe = serde_json::json!({
            "streams": [{ "channels": 2, "tags": { "TITLE": format!("Room\u{7}{}", "x".repeat(200)), "language": "  " } }]
        });
        let stream = &parse_audio_streams(&probe).streams[0];
        let title = stream.title.as_deref().unwrap();
        assert!(title.starts_with("Roomx"));
        assert_eq!(title.chars().count(), MAX_STREAM_TAG_CHARS);
        assert_eq!(stream.language, None);
    }

    #[test]
    fn the_converted_stream_start_after_the_earliest_stream_is_read() {
        let probe = |format: &str, stream: Option<&str>| {
            let mut stream_json = serde_json::json!({ "channels": 2, "sample_rate": "48000" });
            if let Some(start) = stream {
                stream_json["start_time"] = serde_json::json!(start);
            }
            parse_probe(&serde_json::json!({
                "format": { "format_name": "matroska,webm", "start_time": format },
                "streams": [stream_json],
            }))
            .start_offset_micros
        };
        // Audio 0.5 s after the video.
        assert_eq!(probe("0.000000", Some("0.500000")), Some(500_000));
        assert_eq!(probe("1.400000", Some("1.433333")), Some(33_333));
        // The audio is the earliest stream, starts earlier, or has no start.
        assert_eq!(probe("0.500000", Some("0.500000")), None);
        assert_eq!(probe("0.000000", Some("-0.021333")), None);
        assert_eq!(probe("0.000000", None), None);
        assert_eq!(probe("N/A", Some("0.500000")), None);
    }

    #[test]
    fn manifest_records_the_converted_stream_and_the_stream_count() {
        let probed = ProbedSource {
            audio_stream_count: Some(3),
            ..ProbedSource::default()
        };
        let manifest = sample_input(&probed, &[], false);
        assert_eq!(manifest.audio_stream.index, 0);
        assert_eq!(manifest.audio_stream.source_audio_stream_count, Some(3));
        let json = serde_json::to_value(&manifest).unwrap();
        assert_eq!(json["audioStream"]["sourceAudioStreamCount"], 3);
        assert!(schema_errors(&manifest).is_empty());
    }

    #[test]
    fn wav_header_is_read_from_the_file_never_assumed() {
        let dir = TestDir::new();
        let riff = dir.0.join("riff.wav");
        fs::write(&riff, b"RIFF\x24\0\0\0WAVE").unwrap();
        assert_eq!(wav_header_kind(&riff), Ok("riff"));
        let rf64 = dir.0.join("rf64.wav");
        fs::write(&rf64, b"RF64\xff\xff\xff\xffWAVE").unwrap();
        assert_eq!(wav_header_kind(&rf64), Ok("rf64"));
        for (name, bytes) in [
            ("other.wav", &b"OggS...."[..]),
            ("short.wav", &b"RI"[..]),
            ("empty.wav", &b""[..]),
        ] {
            let path = dir.0.join(name);
            fs::write(&path, bytes).unwrap();
            assert!(wav_header_kind(&path).is_err(), "{name}");
        }
        assert!(wav_header_kind(&dir.0.join("missing.wav")).is_err());
    }

    #[test]
    fn records_written_by_1_1_0_still_match_the_schema() {
        // 1.1.0 wrote no audioStream object, no output.wavHeader and no
        // processing.mapArgs; the
        // schema must keep accepting those records.
        let mut record = serde_json::to_value(sample_input(&ProbedSource::default(), &[], false)).unwrap();
        record.as_object_mut().unwrap().remove("audioStream");
        record["output"].as_object_mut().unwrap().remove("wavHeader");
        record["processing"].as_object_mut().unwrap().remove("mapArgs");
        let errors = schema_errors_for_value(&record);
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn a_dm_timeline_matches_the_schema_and_is_absent_for_other_sources() {
        let plain = serde_json::to_value(sample_input(&ProbedSource::default(), &[], false)).unwrap();
        assert!(plain["source"].get("dmTimeline").is_none());

        use crate::dm::tests::{dm_bytes, gsm_format};
        let block = vec![1u8; 520];
        let short = vec![2u8; 100];
        let bytes = dm_bytes(
            &gsm_format(),
            520,
            &[
                (1_763_800_000, &block),
                (0, &block),
                (1_763_800_060, &block),
                (1_763_799_000, &short),
            ],
        );
        let dm = crate::dm::parse_dm_reader(std::io::Cursor::new(&bytes), bytes.len() as u64, &|| false).unwrap();
        let mut manifest = sample_input(&ProbedSource::default(), &[], false);
        manifest.source.dm_timeline = Some(crate::dm::build_timeline(&dm));
        let record = serde_json::to_value(&manifest).unwrap();
        assert!(
            schema_errors_for_value(&record).is_empty(),
            "{:#?}",
            schema_errors_for_value(&record)
        );
        let timeline = &record["source"]["dmTimeline"];
        assert_eq!(timeline["formatTag"], 49);
        assert_eq!(timeline["codecName"], "gsm_ms");
        assert_eq!(timeline["blocksTotal"], 4);
        assert_eq!(timeline["blocksErased"], 1);
        assert_eq!(timeline["shortFinalBlock"], true);
        assert_eq!(timeline["lastBlockBytesDropped"], 35);
        assert_eq!(timeline["stampsMonotonic"], false);
        assert_eq!(timeline["bytesAfterShortBlock"], 0);
        assert_eq!(timeline["truncatedTailBytes"], 0);
        assert!(timeline.get("markerFile").is_none(), "no .bmf, no markerFile");
        assert_eq!(timeline["segments"].as_array().unwrap().len(), 3);
        assert_eq!(timeline["segments"][1]["startSample"], 8 * 320);
        assert_eq!(timeline["segments"][2]["firstStampUtc"], "2025-11-22T08:10:00Z");

        // Older records without the optional fields still validate; unknown
        // fields and malformed stamps do not.
        let mut older = record.clone();
        for field in [
            "lastBlockBytesDropped",
            "segmentsOmitted",
            "warnings",
            "bytesAfterShortBlock",
            "truncatedTailBytes",
        ] {
            older["source"]["dmTimeline"].as_object_mut().unwrap().remove(field);
        }
        assert!(schema_errors_for_value(&older).is_empty());
        let mut drift = record.clone();
        drift["source"]["dmTimeline"]["guessedSpeaker"] = "Witness".into();
        drift["source"]["dmTimeline"]["segments"][0]["firstStampUtc"] = "yesterday".into();
        let errors = schema_errors_for_value(&drift);
        assert!(
            errors.iter().any(|error| error.contains("guessedSpeaker")),
            "{errors:#?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("firstStampUtc")),
            "{errors:#?}"
        );

        // A damaged file read up to its short block, as SoundSync's player
        // reads it; what follows is reported, not read.
        let damaged =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/fixtures/dm/ima-adpcm-damaged.dm");
        let dm = crate::dm::parse_dm(&damaged, &|| false).unwrap();
        let mut manifest = sample_input(&ProbedSource::default(), &[], false);
        manifest.source.dm_timeline = Some(crate::dm::build_timeline(&dm));
        let record = serde_json::to_value(&manifest).unwrap();
        assert!(
            schema_errors_for_value(&record).is_empty(),
            "{:#?}",
            schema_errors_for_value(&record)
        );
        let timeline = &record["source"]["dmTimeline"];
        assert_eq!(timeline["bytesAfterShortBlock"], 2 * 2056 + 108);
        assert_eq!(timeline["truncatedTailBytes"], 0);
        assert_eq!(timeline["shortFinalBlock"], true);
        assert_eq!(timeline["lastBlockBytesDropped"], 10);
        assert_eq!(timeline["warnings"].as_array().unwrap().len(), 2);

        // The owner's sample with its .bmf: marker counts, never marker text.
        let sample = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../scripts/fixtures/dm/vendor-notes-samples/tone_gsm.dm");
        let dm = crate::dm::parse_dm(&sample, &|| false).unwrap();
        let mut timeline = crate::dm::build_timeline(&dm);
        timeline.marker_file = crate::bmf::summary_beside(&sample, &dm);
        let mut manifest = sample_input(&ProbedSource::default(), &[], false);
        manifest.source.dm_timeline = Some(timeline);
        let record = serde_json::to_value(&manifest).unwrap();
        assert!(
            schema_errors_for_value(&record).is_empty(),
            "{:#?}",
            schema_errors_for_value(&record)
        );
        let markers = &record["source"]["dmTimeline"]["markerFile"];
        assert_eq!(markers["status"], "read");
        assert_eq!(markers["entries"], 2);
        assert_eq!(markers["entriesWithText"], 2);
        let text = record.to_string();
        assert!(!text.contains("Objection") && !text.contains("\"Start\""), "{text}");
        let mut drift = record.clone();
        drift["source"]["dmTimeline"]["markerFile"]["text"] = "Objection".into();
        assert!(
            !schema_errors_for_value(&drift).is_empty(),
            "the schema has no place for marker text"
        );
    }

    #[test]
    fn schema_ties_the_wav_header_to_the_output_format() {
        let base = serde_json::to_value(sample_input(&ProbedSource::default(), &[], false)).unwrap();
        assert_eq!(base["processing"]["mapArgs"], serde_json::json!(["-map", "[op1]"]));
        let with = |format: &str, header: serde_json::Value| {
            let mut record = base.clone();
            record["output"]["format"] = format.into();
            record["output"]["wavHeader"] = header;
            schema_errors_for_value(&record)
        };
        assert!(with("wav", "riff".into()).is_empty());
        assert!(with("wav", "rf64".into()).is_empty());
        assert!(with("mp3", serde_json::Value::Null).is_empty());
        for (format, header) in [
            ("wav", serde_json::Value::Null),
            ("mp3", "riff".into()),
            ("flac", "rf64".into()),
            ("opus", "riff".into()),
            ("m4a", "rf64".into()),
        ] {
            let errors = with(format, header.clone());
            assert!(
                errors.iter().any(|error| error.contains("$.output.wavHeader")),
                "{format} {header}: {errors:#?}"
            );
        }
    }

    #[test]
    fn schema_validator_rejects_drift() {
        let mut manifest = sample_input(&ProbedSource::default(), &[], false);
        manifest.source.sha256 = "not-a-digest".into();
        manifest.processing.mode = "merge".into();
        let errors = schema_errors(&manifest);
        assert!(
            errors.iter().any(|error| error.contains("$.source.sha256")),
            "{errors:#?}"
        );
        assert!(
            errors.iter().any(|error| error.contains("$.processing.mode")),
            "{errors:#?}"
        );
    }

    fn ffmpeg_available() -> bool {
        let available = std::process::Command::new("ffmpeg").arg("-version").output().is_ok()
            && std::process::Command::new("ffprobe").arg("-version").output().is_ok();
        if !available {
            // CI sets this so a runner image without FFmpeg fails instead of
            // passing these tests without running them.
            assert!(
                std::env::var_os("DEPOAUDIO_REQUIRE_FFMPEG_TESTS").is_none_or(|value| value != "1"),
                "DEPOAUDIO_REQUIRE_FFMPEG_TESTS=1, but ffmpeg/ffprobe is not on PATH"
            );
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
        }
        available
    }

    /// A synthetic 1 s, 44.1 kHz WAV whose bext TimeReference is 01:00:00.
    fn synthetic_bext_source(dir: &TestDir) -> PathBuf {
        let source = dir.0.join("synthetic_bext.wav");
        let status = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=1:sample_rate=44100")
            .args(["-write_bext", "1", "-metadata", "time_reference=158760000", "-y"])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        source
    }

    fn probe_json(path: &Path) -> serde_json::Value {
        let output = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
                "-select_streams",
                "a:0",
            ])
            .arg(path)
            .output()
            .unwrap();
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Encode `source` with the conversion's own argument shape: input
    /// prelude, optional `-af` filter, then the exact encoder arguments.
    fn encode(source: &Path, filter: Option<&str>, encoder: &[String], output: &Path) {
        let mut command = std::process::Command::new("ffmpeg");
        command
            .args(crate::helpers::safe_ffmpeg_input_prelude())
            .args(["-loglevel", "error", "-i"])
            .arg(source);
        if let Some(filter) = filter {
            command.args(["-af", filter]);
        }
        let status = command.args(encoder).arg("-y").arg(output).status().unwrap();
        assert!(status.success(), "{}", output.display());
    }

    /// Every `time_reference` tag ffprobe reports, in format or stream tags.
    fn time_reference_tags(probe: &serde_json::Value) -> Vec<String> {
        let mut found = Vec::new();
        let mut scopes = vec![&probe["format"]["tags"]];
        if let Some(streams) = probe["streams"].as_array() {
            scopes.extend(streams.iter().map(|stream| &stream["tags"]));
        }
        for scope in scopes {
            if let Some(value) = tag(scope, "time_reference") {
                found.push(value.to_string());
            }
        }
        found
    }

    /// Runs only where FFmpeg is on PATH (CI installs it): FFmpeg copies the
    /// input's `time_reference` tag into FLAC, MP3 and Opus outputs without
    /// rescaling it. Outputs with no bext chunk, and WAV after leading-silence
    /// trim, must carry no start at all, so the export record is the only one.
    #[test]
    fn ffmpeg_drops_the_copied_time_reference_where_no_bext_is_written() {
        if !ffmpeg_available() {
            return;
        }
        let dir = TestDir::new();
        let source = synthetic_bext_source(&dir);
        let probed = parse_probe(&probe_json(&source));
        assert!(probed.timing.is_some());

        // Control: without the clearing argument the tag is copied as is, so
        // the assertions below test something real.
        let copied = dir.0.join("synthetic_copied.flac");
        encode(
            &source,
            None,
            &crate::helpers::output_args("flac", "48000", 192),
            &copied,
        );
        assert_eq!(time_reference_tags(&probe_json(&copied)), vec!["158760000".to_string()]);

        for (index, format) in ["flac", "mp3", "opus"].into_iter().enumerate() {
            for trimmed in [0, 4_410] {
                let mut encoder = crate::helpers::output_args(format, "48000", 192);
                let rate = output_sample_rate(&encoder);
                encoder.extend(start_time_metadata_args(format, probed.timing, rate, trimmed));
                let filter = (trimmed > 0).then(|| crate::ffmpeg::leading_trim_filters(trimmed).join(","));
                let output = dir.0.join(format!(
                    "synthetic_out_{index}_{trimmed}{}",
                    crate::helpers::output_ext(format)
                ));
                encode(&source, filter.as_deref(), &encoder, &output);
                let probe = probe_json(&output);
                assert_eq!(
                    time_reference_tags(&probe),
                    Vec::<String>::new(),
                    "{format} trimmed={trimmed}"
                );
                assert_eq!(parse_probe(&probe).timing, None, "{format} trimmed={trimmed}");
            }
        }
    }

    /// Runs only where FFmpeg is on PATH (CI installs it): proves the exact
    /// arguments carry a rescaled bext TimeReference into a WAV output.
    #[test]
    fn ffmpeg_carries_the_rescaled_time_reference_into_wav() {
        if !ffmpeg_available() {
            return;
        }
        let dir = TestDir::new();
        let source = synthetic_bext_source(&dir);
        let probed = parse_probe(&probe_json(&source));
        assert_eq!(
            probed.timing,
            Some(SourceTiming {
                time_reference_samples: 158_760_000,
                sample_rate: 44_100
            })
        );

        let mut encoder = crate::helpers::output_args("wav", "48000", 192);
        assert!(!encoder.iter().any(|arg| arg == "-rf64"));
        let rate = output_sample_rate(&encoder);
        encoder.extend(start_time_metadata_args("wav", probed.timing, rate, 0));
        let output = dir.0.join("synthetic_bext_out.wav");
        encode(&source, None, &encoder, &output);
        let carried = parse_probe(&probe_json(&output));
        assert_eq!(
            carried.timing,
            Some(SourceTiming {
                time_reference_samples: 172_800_000,
                sample_rate: 48_000
            })
        );
        // After 0.1 s of leading-silence trim (4,410 samples at 44.1 kHz) the
        // output starts at 01:00:00.1: 172,804,800 samples at 48 kHz.
        let mut trimmed_encoder = crate::helpers::output_args("wav", "48000", 192);
        trimmed_encoder.extend(start_time_metadata_args("wav", probed.timing, rate, 4_410));
        let trimmed_output = dir.0.join("synthetic_bext_trimmed.wav");
        let trim = crate::ffmpeg::leading_trim_filters(4_410).join(",");
        encode(&source, Some(&trim), &trimmed_encoder, &trimmed_output);
        assert_eq!(
            parse_probe(&probe_json(&trimmed_output)).timing,
            Some(SourceTiming {
                time_reference_samples: 172_804_800,
                sample_rate: 48_000
            })
        );

        // Without an RF64 option the file is plain RIFF with no JUNK chunk.
        assert_eq!(wav_header_kind(&output), Ok("riff"));
        assert!(!fs::read(&output).unwrap()[..64].windows(4).any(|w| w == b"JUNK"));

        // `-rf64 auto` on a small file: still RIFF, but FFmpeg reserves a
        // JUNK chunk before `fmt `, which is why the app adds it only to a
        // WAV that may pass 4 GiB.
        let mut auto = encoder.clone();
        auto.extend(crate::helpers::wav_rf64_args(None));
        let reserved = dir.0.join("synthetic_bext_auto.wav");
        encode(&source, None, &auto, &reserved);
        assert_eq!(wav_header_kind(&reserved), Ok("riff"));
        assert_eq!(&fs::read(&reserved).unwrap()[12..16], b"JUNK");
        assert_eq!(parse_probe(&probe_json(&reserved)).timing, carried.timing);

        // The header reader and the bext path on a real RF64 file. Writing
        // 4 GiB in a test is not practical, so `-rf64 always` stands in for
        // what `auto` does once a file outgrows RIFF.
        let mut forced = encoder.clone();
        forced.extend(["-rf64".to_string(), "always".to_string()]);
        let rf64 = dir.0.join("synthetic_bext_rf64.wav");
        encode(&source, None, &forced, &rf64);
        assert_eq!(wav_header_kind(&rf64), Ok("rf64"));
        assert_eq!(parse_probe(&probe_json(&rf64)).timing, carried.timing);
    }
}
