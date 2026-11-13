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
/// ffprobe `-show_format -show_streams -select_streams a:0` JSON document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProbedSource {
    pub channels: Option<u32>,
    pub timing: Option<SourceTiming>,
}

fn tag<'a>(tags: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    tags.as_object()?
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .and_then(|(_, value)| value.as_str())
}

pub(crate) fn parse_probe(probe: &serde_json::Value) -> ProbedSource {
    let stream = &probe["streams"][0];
    let channels = stream["channels"]
        .as_u64()
        .and_then(|count| u32::try_from(count).ok())
        .filter(|count| *count > 0);
    let sample_rate = stream["sample_rate"]
        .as_str()
        .and_then(|rate| rate.trim().parse::<u32>().ok())
        .filter(|rate| *rate > 0);
    // A `time_reference` tag in any other container (FLAC, MP3, Ogg, MP4) is
    // a plain tag, not a bext chunk: FFmpeg copies the tag from a WAV input
    // into those outputs without rescaling it, so it is not a trusted start.
    let from_wav = probe["format"]["format_name"].as_str() == Some("wav");
    let time_reference = tag(&probe["format"]["tags"], "time_reference")
        .filter(|_| from_wav)
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .and_then(|value| value.parse::<u64>().ok());
    let timing = match (time_reference, sample_rate) {
        (Some(time_reference_samples), Some(sample_rate)) => Some(SourceTiming {
            time_reference_samples,
            sample_rate,
        }),
        _ => None,
    };
    ProbedSource { channels, timing }
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

/// Leading-silence trim (`silenceremove`) moves the output's first sample
/// later by an amount FFmpeg does not report, so the source start no longer
/// describes the output. Every other filter keeps the timeline.
pub(crate) fn filters_shift_start(filters: &[String]) -> bool {
    filters.iter().any(|filter| filter.contains("silenceremove"))
}

/// The sample rate the encoder writes, read from the exact `-ar` argument.
pub(crate) fn output_sample_rate(encoder_args: &[String]) -> Option<u32> {
    encoder_args
        .windows(2)
        .find(|pair| pair[0] == "-ar")
        .and_then(|pair| pair[1].parse().ok())
}

/// FFmpeg arguments that carry the start into a WAV output's own bext chunk
/// (`-write_bext 1` plus the rescaled `time_reference`). Empty when the output
/// is not WAV, the start is unknown, or a filter moved the start.
pub(crate) fn bext_args(
    format: &str,
    timing: Option<SourceTiming>,
    output_rate: Option<u32>,
    shifted: bool,
) -> Vec<String> {
    match (format, timing, output_rate, shifted) {
        ("wav", Some(timing), Some(rate), false) => vec![
            "-write_bext".into(),
            "1".into(),
            "-metadata".into(),
            format!("time_reference={}", rescale_time_reference(timing, rate)),
        ],
        _ => Vec::new(),
    }
}

/// The start-time arguments for one output. A WAV output with a known,
/// unshifted start gets its own bext chunk (`bext_args`). Every other output
/// gets `-metadata time_reference=`, which deletes the tag: FFmpeg would
/// otherwise copy the source's `time_reference` into FLAC, MP3 and Opus
/// outputs unrescaled, and the output would contradict its export record.
pub(crate) fn start_time_metadata_args(
    format: &str,
    timing: Option<SourceTiming>,
    output_rate: Option<u32>,
    shifted: bool,
) -> Vec<String> {
    let bext = bext_args(format, timing, output_rate, shifted);
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
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OutputRecord {
    pub file_name: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub format: String,
    pub sample_rate: Option<u32>,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessingRecord {
    pub mode: String,
    pub filter_option: Option<&'static str>,
    pub filter_chain: Option<String>,
    pub encoder_args: Vec<String>,
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
    pub source_channel_index: Option<u32>,
    pub label: Option<String>,
}

fn bounded_label(label: &str) -> String {
    label.trim().chars().take(MAX_LABEL_CHARS).collect()
}

pub(crate) fn start_time_record(
    timing: Option<SourceTiming>,
    output_rate: Option<u32>,
    shifted: bool,
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
    let output_reference = match (output_rate, shifted) {
        (Some(rate), false) => Some(rescale_time_reference(timing, rate)),
        _ => None,
    };
    let written = !bext_args(format, Some(timing), output_rate, shifted).is_empty();
    let note = if shifted {
        "Leading-silence trim removed audio from the start, so the output starts later than the source by an unrecorded amount. The output carries no start time; the source start is kept here for reference only."
    } else if written {
        "The output WAV carries the same start in its own bext chunk, rescaled to the output sample rate."
    } else {
        "The output starts at the source start. This format has no bext chunk, and DepoAudio removes the copied time_reference tag from it, so the start is recorded here only."
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
    pub shifted: bool,
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
        },
        processing: ProcessingRecord {
            mode: facts.mode.into(),
            filter_option: facts.filter_option,
            filter_chain: facts.filter_chain,
            encoder_args: facts.encoder_args,
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
        start_time: start_time_record(input.probed.timing, sample_rate, input.shifted, facts.format),
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

    fn sample_input(probed: &ProbedSource, labels: &[String], shifted: bool) -> ExportManifest {
        let input = ManifestInput {
            app_version: "1.0.3".into(),
            created_at: "2026-11-13T09:00:00Z".into(),
            source: FileRecord {
                file_name: "synthetic_session.wav".into(),
                sha256: "a".repeat(64),
                size_bytes: 1024,
            },
            probed,
            labels,
            shifted,
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
                filter_chain: Some("[0:a]asplit=2[sp0][sp1];[sp0]pan=mono|c0=c0[op0];[sp1]pan=mono|c0=c1[op1]".into()),
                encoder_args: vec!["-acodec".into(), "pcm_s16le".into(), "-ar".into(), "48000".into()],
                source_channel_index: Some(1),
                label: Some("Witness".into()),
            },
        )
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
            start_time_metadata_args("wav", timing, Some(16_000), false),
            bext_args("wav", timing, Some(16_000), false)
        );
        for format in ["flac", "mp3", "opus", "m4a"] {
            assert_eq!(
                start_time_metadata_args(format, timing, Some(48_000), false),
                clear,
                "{format}"
            );
            assert_eq!(
                start_time_metadata_args(format, timing, Some(48_000), true),
                clear,
                "{format}"
            );
            assert_eq!(
                start_time_metadata_args(format, None, Some(48_000), false),
                clear,
                "{format}"
            );
        }
        assert_eq!(start_time_metadata_args("wav", timing, Some(48_000), true), clear);
        assert_eq!(start_time_metadata_args("wav", None, Some(48_000), false), clear);
        assert_eq!(start_time_metadata_args("wav", timing, None, false), clear);
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
    fn bext_is_written_only_for_wav_with_a_known_unshifted_start() {
        let timing = Some(SourceTiming {
            time_reference_samples: 48_000,
            sample_rate: 48_000,
        });
        assert_eq!(
            bext_args("wav", timing, Some(16_000), false),
            vec!["-write_bext", "1", "-metadata", "time_reference=16000"]
        );
        assert!(bext_args("wav", timing, Some(16_000), true).is_empty());
        assert!(bext_args("wav", None, Some(16_000), false).is_empty());
        assert!(bext_args("flac", timing, Some(16_000), false).is_empty());
        assert!(bext_args("wav", timing, None, false).is_empty());
    }

    #[test]
    fn only_silence_removal_shifts_the_start() {
        assert!(!filters_shift_start(&[
            "highpass=f=80".into(),
            "loudnorm=I=-16:TP=-1.5:LRA=11".into(),
            "afade=t=in:d=0.5".into()
        ]));
        assert!(filters_shift_start(&[
            "silenceremove=start_periods=1:start_duration=0.3:start_threshold=-50dB:start_mode=any".into()
        ]));
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

        let trimmed = sample_input(&probed, &labels, true);
        assert!(trimmed.start_time.known);
        assert_eq!(trimmed.start_time.output_time_reference_samples, None);
        assert!(!trimmed.start_time.written_to_output);

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
    // items, minimum, maximum, pattern). It fails on any other keyword so the
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
                "$schema" | "$id" | "title" | "description" => {}
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
                    for name in rule.as_array().unwrap() {
                        let name = name.as_str().unwrap();
                        if value.get(name).is_none() {
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

        let trim_filter = "silenceremove=start_periods=1:start_duration=0.3:start_threshold=-50dB:start_mode=any";
        let cases: [(&str, Option<&str>); 7] = [
            ("flac", None),
            ("mp3", None),
            ("opus", None),
            ("flac", Some(trim_filter)),
            ("mp3", Some(trim_filter)),
            ("opus", Some(trim_filter)),
            ("wav", Some(trim_filter)),
        ];
        for (index, (format, filter)) in cases.into_iter().enumerate() {
            let filters: Vec<String> = filter.iter().map(|f| f.to_string()).collect();
            let shifted = filters_shift_start(&filters);
            let mut encoder = crate::helpers::output_args(format, "48000", 192);
            let rate = output_sample_rate(&encoder);
            encoder.extend(start_time_metadata_args(format, probed.timing, rate, shifted));
            let output = dir
                .0
                .join(format!("synthetic_out_{index}{}", crate::helpers::output_ext(format)));
            encode(&source, filter, &encoder, &output);
            let probe = probe_json(&output);
            assert_eq!(
                time_reference_tags(&probe),
                Vec::<String>::new(),
                "{format} filter={filter:?}"
            );
            assert_eq!(parse_probe(&probe).timing, None, "{format} filter={filter:?}");
            let record = start_time_record(probed.timing, rate, shifted, format);
            assert!(!record.written_to_output, "{format} filter={filter:?}");
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
        let rate = output_sample_rate(&encoder);
        encoder.extend(start_time_metadata_args("wav", probed.timing, rate, false));
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
    }
}
