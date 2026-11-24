use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::dcr::DcrInfo;
use crate::ffmpeg::{
    build_proc_filters_with_gain, probe_channels_cancellable, probe_duration_cancellable,
    probe_manifest_source_cancellable, run_ffmpeg_with_timeout,
};
use crate::helpers::{
    basename, output_args, output_ext, prepare_audio_feed_with_cancel, safe_ffmpeg_input_prelude, safe_label,
    wav_estimated_bytes, wav_rf64_args, AudioFeed, FeedOptions,
};
use crate::manifest::{self, FileRecord, ManifestInput, OutputFacts};
use crate::safety::TempFile;
use crate::types::{ConvertJob, OutputFile, ProgressEvent, CONVERSION_CANCELLED_MESSAGE};
/// FTR's supported registry and the analysis engine both cap court recordings
/// at 16 channels. Reject corrupt or unsupported headers before allocating
/// labels, filters, or output reservations from their channel count.
const MAX_CONVERSION_CHANNELS: u32 = 16;

/// Immutable view of one conversion batch's cancellation generation.
#[derive(Clone)]
pub(crate) struct ConversionCancel {
    epoch: Arc<AtomicU64>,
    generation: u64,
}

impl ConversionCancel {
    pub(crate) fn new(epoch: Arc<AtomicU64>, generation: u64) -> Self {
        Self { epoch, generation }
    }

    pub(crate) fn cancelled(&self) -> bool {
        self.epoch.load(Ordering::Acquire) != self.generation
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        if self.cancelled() {
            Err(CONVERSION_CANCELLED_MESSAGE.into())
        } else {
            Ok(())
        }
    }
}

/// Deletes every reserved/output path unless the complete result is explicitly
/// committed. This covers cancellation and all early-return branches, not only
/// FFmpeg's conventional non-zero exit path.
#[derive(Default)]
struct OutputCleanup {
    paths: Vec<PathBuf>,
    committed: bool,
}

impl OutputCleanup {
    fn track(&mut self, path: PathBuf) {
        self.paths.push(path);
    }

    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for OutputCleanup {
    fn drop(&mut self) {
        if !self.committed {
            for path in &self.paths {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn track_planned(cleanup: &mut OutputCleanup, planned: &[PlannedOutput]) {
    for output in planned {
        cleanup.track(output.path.clone());
        cleanup.track(output.manifest.clone());
    }
}

// ── Filtergraph builders (pure) ──────────────────────────────────────────────
//
// These produce the exact FFmpeg filter strings for each output mode. Their
// output format is a contract: golden-master tests below pin it down.

/// Opus bitrate per mono stream of an SGMCA output (48 kHz, as the notes say
/// CATalyst encodes).
const SGMCA_KBPS_PER_CHANNEL: u32 = 32;

/// Peak guard for Mix to Stereo with normalize off. FFmpeg's alimiter looks
/// 5 ms ahead and, by default (`latency=0`), outputs that lookahead as a
/// delay: the output keeps the source's length, so the audio starts about
/// 5 ms late and the last 5 ms of the source are cut. `latency=1` removes the
/// delay so the output lines up sample for sample with the source. The option
/// is in every bundled FFmpeg (5.1 or newer); an FFmpeg without it fails the
/// conversion instead of shifting the audio.
const STEREO_PEAK_LIMITER: &str = "alimiter=limit=0.97:level=false:latency=1";

/// Filters a conversion chain may hold, each measured not to delay or move
/// the audio (with FFmpeg 6.1, by the position of clicks in a test file):
/// the mixers and gain, de-clip, high-pass, loudnorm (which compensates its
/// own look-ahead), fades, sample-format conversion, `amerge` (which joins
/// an FTR recording's channel files, sample for sample, in channel order;
/// measured by `an_ftr_source_on_the_split_route_lines_up_sample_for_sample`),
/// and alimiter only with `latency=1` (see `STEREO_PEAK_LIMITER`).
/// Leading-silence trim moves the start on purpose, by a measured sample
/// count that the export record holds (`ffmpeg::leading_trim_filters`), so
/// `atrim` is allowed only as `atrim=start_sample=N` and `asetpts` only as
/// the two timeline resets trim and fade use. Any other filter is refused,
/// so one that delays the audio cannot be added without being measured here
/// first.
const TIMING_SAFE_FILTERS: [&str; 12] = [
    "amerge", "pan", "volume", "asplit", "aformat", "adeclip", "highpass", "loudnorm", "afade", "atrim", "asetpts",
    "alimiter",
];

/// Stereo downmix: a unity-gain SUM of all channels on both L and R.
/// `vols` are per-channel volumes already multiplied by 1/num_ch; missing
/// entries fall back to an equal-weight mix.
///
/// `aformat=sample_fmts=fltp` first: on 16-bit input FFmpeg's pan mixes in
/// 16-bit integers, and a weighted sum past full scale (a channel turned up,
/// or auto-level gains, on overlapping loud speech) wraps around to the
/// opposite polarity instead of reaching the limiter. In float the sum keeps
/// its value until the limiter or loudnorm brings it back under full scale.
fn stereo_pan_filter(num_ch: u32, vols: &[f64]) -> String {
    let weight = 1.0 / num_ch as f64;
    let scale = num_ch as f64; // compensate for per-channel weight
    let weights: Vec<String> = (0..num_ch)
        .map(|i| {
            let v = vols.get(i as usize).copied().unwrap_or(weight);
            format!("{:.4}*c{}", v, i)
        })
        .collect();
    let mix = weights.join("+");
    format!(
        "aformat=sample_fmts=fltp,pan=stereo|c0={}|c1={},volume={:.1}",
        mix, mix, scale
    )
}

/// Per-channel output labels for split mode: the user's label, sanitized for
/// filenames, falling back to chN when empty.
fn split_labels(num_ch: u32, labels: &[String]) -> Vec<String> {
    (0..num_ch as usize)
        .map(|i| {
            let raw = labels.get(i).map(|s| s.as_str()).unwrap_or("");
            let sl = safe_label(raw);
            if sl.is_empty() {
                format!("ch{}", i + 1)
            } else {
                sl
            }
        })
        .collect()
}

/// Split mode filter_complex: asplit + pan=mono per channel (channelsplit
/// requires the actual channel layout, which breaks mono and 4-channel court
/// recordings). Auto-level injects EACH channel's own gain right after the
/// channel is isolated — a single averaged gain would leave imbalance in place.
#[cfg(test)]
fn split_filter_complex(num_ch: u32, auto_level: bool, channel_gains: Option<&Vec<f64>>, proc: &[String]) -> String {
    split_filter_complex_from(
        (String::new(), crate::helpers::source_audio_filter_input()),
        num_ch,
        auto_level,
        channel_gains,
        proc,
    )
}

/// [`split_filter_complex`] reading the converted stream from `source`: the
/// filtergraph prefix and label from `AudioFeed::filter_source` (`[0:a:0]`
/// for one input, the `amerge` of an FTR recording's channel files otherwise).
fn split_filter_complex_from(
    source: (String, String),
    num_ch: u32,
    auto_level: bool,
    channel_gains: Option<&Vec<f64>>,
    proc: &[String],
) -> String {
    let (prefix, input) = source;
    let sp_tags: Vec<String> = (0..num_ch as usize).map(|i| format!("sp{}", i)).collect();
    // `[0:a:0]`: the converted stream, never FFmpeg's choice among tracks.
    let split_str = format!("{prefix}{input}asplit={}[{}]", num_ch, sp_tags.join("]["));
    let per_ch_proc = if proc.is_empty() {
        String::new()
    } else {
        format!(",{}", proc.join(","))
    };
    let chain: Vec<String> = (0..num_ch as usize)
        .map(|i| {
            let gain_str = if auto_level {
                match channel_gains.and_then(|g| g.get(i)).copied() {
                    Some(g) if (g - 1.0).abs() > 0.01 => format!(",volume={:.4}", g),
                    _ => String::new(),
                }
            } else {
                String::new()
            };
            format!("[sp{}]pan=mono|c0=c{}{}{}[op{}]", i, i, gain_str, per_ch_proc, i)
        })
        .collect();
    std::iter::once(split_str).chain(chain).collect::<Vec<_>>().join(";")
}

/// One reserved output and its export record (`<output>.depoaudio.json`).
#[derive(Debug, Clone)]
struct PlannedOutput {
    path: PathBuf,
    manifest: PathBuf,
    /// Zero-based source channel of a split file.
    channel_index: Option<u32>,
    label: Option<String>,
}

/// Reserve a set of outputs, each with its export record, as one operation
/// from the caller's point of view. If any reservation fails, remove every
/// placeholder already acquired so a split conversion never leaves a partial
/// set behind.
fn reserve_output_paths(
    paths: impl IntoIterator<Item = (PathBuf, Option<u32>, Option<String>)>,
) -> Result<Vec<PlannedOutput>, String> {
    let mut reserved: Vec<PlannedOutput> = Vec::new();
    for (path, channel_index, label) in paths {
        match manifest::reserve_output_and_manifest(&path) {
            Ok((path, manifest)) => reserved.push(PlannedOutput {
                path,
                manifest,
                channel_index,
                label,
            }),
            Err(error) => {
                for planned in &reserved {
                    let _ = fs::remove_file(&planned.path);
                    let _ = fs::remove_file(&planned.manifest);
                }
                return Err(error);
            }
        }
    }
    Ok(reserved)
}

fn validate_conversion_channels(channels: u32) -> Result<u32, String> {
    match channels {
        1..=MAX_CONVERSION_CHANNELS => Ok(channels),
        0 => Err("The recording reports zero audio channels and cannot be converted.".into()),
        count => Err(format!(
            "The recording reports {count} audio channels; DepoAudio supports at most {MAX_CONVERSION_CHANNELS}. The file header may be corrupt or use an unsupported layout."
        )),
    }
}

/// Output formats whose Keep Original conversion is refused for a source with
/// more than two channels. FFmpeg's MP3 encoder holds at most two, so the
/// channels are folded into stereo; AAC (M4A) and Opus treat a channel the
/// layout marks LFE (the fourth of a 3.1 WAV, the fourth of 5.1) as a
/// low-pass effects channel, which removes speech from it; Opus also has no
/// mapping for several common layouts. Keep Original promises every channel
/// unchanged, so these are refused before any output is written.
const KEEP_STEREO_ONLY_FORMATS: [&str; 3] = ["mp3", "m4a", "opus"];

fn keep_needs_channel_count(job: &ConvertJob) -> bool {
    job.mode == "keep" && KEEP_STEREO_ONLY_FORMATS.contains(&job.format.as_str())
}

fn validate_keep_channels(format: &str, channels: Option<u32>) -> Result<(), String> {
    if !KEEP_STEREO_ONLY_FORMATS.contains(&format) {
        return Ok(());
    }
    let name = match format {
        "mp3" => "MP3",
        "m4a" => "M4A",
        _ => "Opus",
    };
    match channels {
        Some(1 | 2) => Ok(()),
        Some(count) => Err(format!(
            "Keep Original cannot write this {count}-channel recording as {name} without losing channels: {name} here keeps at most 2 channels unchanged, and FFmpeg would fold or filter the rest. Nothing was converted. Choose WAV, FLAC or SGMCA to keep every channel, or Split Channels or Mix to Stereo."
        )),
        None => Err(format!(
            "Keep Original to {name} needs the recording's channel count, and it could not be read, so nothing was converted. Choose WAV, FLAC or SGMCA, or Split Channels or Mix to Stereo."
        )),
    }
}

fn validate_finite_range(label: &str, value: f64, min: f64, max: f64) -> Result<(), String> {
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(format!("{label} must be a finite value from {min} to {max}."));
    }
    Ok(())
}

fn validate_convert_request(job: &ConvertJob) -> Result<(), String> {
    if job.denoise || job.enhance || job.dereverb {
        return Err(format!(
            "Learned-model processing is not included in DepoAudio v{}. Turn off Noise Removal, Enhance Clarity, and Reduce Room Echo.",
            env!("CARGO_PKG_VERSION")
        ));
    }
    if !matches!(job.mode.as_str(), "stereo" | "keep" | "split") {
        return Err(format!("Unsupported conversion mode: {}", job.mode));
    }
    if !matches!(job.format.as_str(), "wav" | "mp3" | "flac" | "opus" | "m4a" | "sgmca") {
        return Err(format!("Unsupported output format: {}", job.format));
    }
    validate_finite_range("Maximum input size", job.max_file_size_gb, f64::MIN_POSITIVE, 20.0)?;
    validate_finite_range("High-pass cutoff", job.hpf_cutoff, 20.0, 500.0)?;
    validate_finite_range("Normalization loudness", job.normalize_lufs, -70.0, -5.0)?;
    validate_finite_range("Normalization true peak", job.normalize_tp, -20.0, 0.0)?;
    validate_finite_range("Silence threshold", job.silence_thresh, -100.0, 0.0)?;
    if job
        .chan_vols
        .iter()
        .any(|volume| !volume.is_finite() || !(0.0..=2.0).contains(volume))
    {
        return Err("Channel volumes must be finite values from 0 to 2.".into());
    }
    if job.mode == "keep" && job.auto_level {
        return Err(
            "Auto-leveling is unavailable in Keep Original mode because that mode must preserve the channel layout without applying one shared gain. Choose Mix to Stereo or Split Channels."
                .into(),
        );
    }
    Ok(())
}

// ── Conversion orchestration ─────────────────────────────────────────────────

pub(crate) async fn do_convert(
    app: &AppHandle,
    job: &ConvertJob,
    cancel: &ConversionCancel,
) -> Result<Vec<OutputFile>, String> {
    cancel.check()?;
    validate_convert_request(job)?;
    // Safety checks
    let src = Path::new(&job.src_path);
    let max_bytes = (job.max_file_size_gb * 1024.0 * 1024.0 * 1024.0) as u64;
    crate::safety::check_file_safe_with_limit(src, max_bytes)?;
    if job.fade {
        crate::safety::validate_fade_dur(job.fade_dur)?;
    }
    crate::safety::validate_rate(&job.rate)?;
    cancel.check()?;

    let fmt = crate::helpers::detect_format_blocking(job.src_path.clone())
        .await
        .ok_or("Unrecognised file format")?;

    if fmt.handler == "rejected" {
        return Err(fmt.note.unwrap_or_else(|| "This format cannot be converted.".into()));
    }
    let options = crate::ffmpeg::feed_options(app, src).await;
    cancel.check()?;
    let (feed, source) = prepare_on_blocking_thread(app, src, options, cancel).await?;
    let (job, source) = with_dcr_facts(job, source, feed.dcr());
    let job = &job;

    let error = match do_convert_inner(app, job, &feed, cancel, source.clone()).await {
        Ok(outputs) => return Ok(outputs),
        Err(InnerError::Other(error)) => return Err(error),
        Err(InnerError::Decode(error)) => error,
    };
    // FFmpeg's native FTR decoder failed on a file whose layout looked right
    // for it: split the channels and decode them instead. The export record
    // names the route and gives "native-decode-failed" as the reason.
    if let Some(message) = plain_trm_decode_error(&feed, &error) {
        return Err(message);
    }
    if !feed.used_native_ftr() || cancel.cancelled() {
        return Err(error);
    }
    drop(feed);
    let preparation_cancel = cancel.clone();
    let feed = crate::helpers::prepare_audio_feed_after_native_failure(
        app,
        src.to_path_buf(),
        Arc::new(move || preparation_cancel.cancelled()),
    )
    .await?;
    cancel.check()?;
    // The original was fingerprinted once already; only the route changed.
    let fallback_source = FileRecord {
        trm: feed.trm().cloned(),
        ..source
    };
    do_convert_inner(app, job, &feed, cancel, fallback_source)
        .await
        .map_err(|fallback| {
            format!(
                "{} (FFmpeg's native FTR decoder failed first: {error})",
                fallback.into_message()
            )
        })
}

/// For an FTR recording whose MP2, MP3 or WMA audio FFmpeg read as it is and
/// failed to decode: the message naming the codec.
fn plain_trm_decode_error(feed: &AudioFeed, error: &str) -> Option<String> {
    feed.trm()
        .filter(|route| route.decode_path == crate::trm::DecodePath::Plain)
        .map(|route| crate::trm::plain_decode_error(&route.info, error))
}

/// Why `do_convert_inner` stopped: a decode failure is worth one retry by
/// another FTR route; anything else ends the conversion.
enum InnerError {
    Decode(String),
    Other(String),
}

impl InnerError {
    fn into_message(self) -> String {
        match self {
            InnerError::Decode(message) | InnerError::Other(message) => message,
        }
    }
}

impl From<String> for InnerError {
    fn from(message: String) -> Self {
        InnerError::Other(message)
    }
}

async fn prepare_on_blocking_thread(
    app: &AppHandle,
    src: &Path,
    options: FeedOptions,
    cancel: &ConversionCancel,
) -> Result<(AudioFeed, FileRecord), String> {
    let preparation_cancel = cancel.clone();
    let preparation_src = src.to_path_buf();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        prepare_source(&preparation_src, options, &move || preparation_cancel.cancelled())
    })
    .await
    .map_err(|error| format!("Audio preparation task failed: {error}"))?;
    cancel.check()?;
    let (feed, source) = prepared?;
    // A DCR whose channels FFmpeg still has to join.
    let merge_cancel = cancel.clone();
    let feed = crate::helpers::finish_prepared_feed(app, feed, &move || merge_cancel.cancelled()).await?;
    cancel.check()?;
    Ok((feed, source))
}

/// Prepare the selected file for FFmpeg (a Direct Audio SGMCA is read in place
/// through a small temporary concat script naming each recording session's
/// byte range, and its layout read; a `.dm` has its audio read into a
/// temporary WAV, a Liberty / BIS DCR is demuxed into a temporary WAV; an FTR
/// recording is read through and, when FFmpeg's native decoder cannot take
/// it, split into temporary per-channel files; every other format is used as
/// is) and fingerprint the original file as the person selected it, before
/// any preparation, for the export record (DA-08). Temporary files are in the
/// private work folder and live as long as the returned feed.
fn prepare_source(
    src: &Path,
    options: FeedOptions,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(AudioFeed, FileRecord), String> {
    let feed = prepare_audio_feed_with_cancel(src, options, Some(cancelled))?;
    let (sha256, size_bytes) = manifest::sha256_file(src, cancelled)?;
    let source = FileRecord {
        file_name: basename(&src.to_string_lossy()),
        sha256,
        size_bytes,
        trm: feed.trm().cloned(),
        trm_inspection_error: feed.trm_inspection_error().map(str::to_string),
        sgmca: feed.sgmca().map(manifest::sgmca_source_record),
        dcr: None,
        dm_timeline: feed.dm_timeline().cloned(),
    };
    Ok((feed, source))
}

/// For a DCR source: split outputs and the export record's labels use the
/// channel names read from the recording ("Ch N" where it names none),
/// in place of the queue-wide labels, and the record gets the `source.dcr`
/// object. The Convert queue says so before the conversion starts.
fn with_dcr_facts(job: &ConvertJob, mut source: FileRecord, dcr: Option<&DcrInfo>) -> (ConvertJob, FileRecord) {
    let mut job = job.clone();
    if let Some(info) = dcr {
        job.labels = info.channel_names.clone();
        source.dcr = Some(info.record());
    }
    (job, source)
}

/// What the probes found before the FFmpeg command is planned.
struct ConversionProbes {
    /// Channel count of the input; probed only for Mix to Stereo and Split.
    channels: Option<u32>,
    /// Per-channel auto-level gains, when auto-level analysis ran.
    channel_gains: Option<Vec<f64>>,
    /// Shared processing filters (trim, de-clip, HPF, loudnorm, fade).
    proc: Vec<String>,
    /// Where the fade-out in `proc` was placed, for the export record.
    fade_out: Option<manifest::FadeOutRecord>,
    /// What the trim in `proc` removes, for the export record and the start.
    leading_trim: Option<manifest::LeadingTrimRecord>,
    /// Channel count, rate and start time for the export record.
    probed: manifest::ProbedSource,
    /// Duration of the converted stream, for the WAV size estimate that
    /// decides on `-rf64 auto`.
    duration_secs: Option<f64>,
    /// The layout of an SGMCA source, read before conversion: its recording
    /// sessions and their start times.
    sgmca_source: Option<crate::sgmca::SgmcaInfo>,
    /// Samples the converted stream decodes to at its own rate
    /// (`probed.sample_rate`), counted only when an SGMCA output needs the
    /// source's exact length and no header states it
    /// (`needs_source_sample_count`).
    source_samples: Option<u64>,
}

/// Why a conversion to SGMCA stops when the source's length is not known.
const SOURCE_LENGTH_UNKNOWN_MESSAGE: &str = "DepoAudio could not count the samples in this recording, so it cannot check that the SGMCA output keeps every one of them. Nothing was converted.";

/// Whether a conversion to SGMCA must count the source's samples before
/// it runs: every SGMCA output is checked against the source's exact
/// length, which an SGMCA source states in its pages and a PCM WAV in its
/// data chunk; any other source is decoded once and counted. After a
/// leading-silence trim that removed samples the output is not held to the
/// source's length (see `plan_sgmca_output`), so nothing is counted.
fn needs_source_sample_count(job: &ConvertJob, feed: &AudioFeed, sgmca_source: bool, trimmed: bool) -> bool {
    job.format == "sgmca" && !sgmca_source && !trimmed && feed_pcm_wav_frames(feed).is_none()
}

/// `pcm_wav_frames` of a feed FFmpeg reads as one file; None for the
/// channel files of an FTR recording, which `amerge` joins.
fn feed_pcm_wav_frames(feed: &AudioFeed) -> Option<(u64, u32)> {
    if feed.is_merged() {
        None
    } else {
        pcm_wav_frames(feed.path())
    }
}

/// Whether leading-silence trim removed any samples, which moves every later
/// sample earlier in the output by the recorded count.
fn trim_moves_start(trim: Option<&manifest::LeadingTrimRecord>) -> bool {
    trim.is_some_and(|trim| trim.samples > 0)
}

/// The exact FFmpeg command for one conversion and the outputs it writes.
struct ConversionPlan {
    planned: Vec<PlannedOutput>,
    /// Every FFmpeg argument after the program name.
    args: Vec<String>,
    filter_option: Option<&'static str>,
    filter_chain: Option<String>,
    /// Encoder plus start-time metadata arguments, shared by every output.
    encoder_args: Vec<String>,
    /// The `-map` arguments of each output, in `planned` order: the source
    /// stream (`-map 0:a:0`) for Mix to Stereo and Keep Original, the
    /// channel's filter branch (`-map [opN]`) for Split.
    map_args: Vec<Vec<String>>,
    /// For an SGMCA output: its segments and where their start times come
    /// from, applied when each output is wrapped in its Direct Audio header.
    sgmca_output: Option<SgmcaOutputPlan>,
    fade_out: Option<manifest::FadeOutRecord>,
    leading_trim: Option<manifest::LeadingTrimRecord>,
}

/// Pure core of a conversion once the input has been probed: reserve every
/// output with its export record (tracked by `cleanup` as soon as reserved)
/// and build the exact FFmpeg arguments. No process runs here, so tests can
/// drive the same plan with FFmpeg from PATH.
fn plan_conversion(
    job: &ConvertJob,
    feed: &AudioFeed,
    probes: &ConversionProbes,
    cleanup: &mut OutputCleanup,
) -> Result<ConversionPlan, String> {
    let base = Path::new(&job.src_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let out_dir = if job.out_dir.is_empty() {
        Path::new(&job.src_path)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf()
    } else {
        PathBuf::from(&job.out_dir)
    };

    let ext = output_ext(&job.format);
    let mut out_codec = output_args(&job.format, &job.rate, job.mp3_bitrate);
    if job.format == "wav" {
        // RF64 only for a WAV that may pass 4 GiB (see `wav_rf64_args`).
        let channels_per_output = match job.mode.as_str() {
            "stereo" => Some(2),
            "split" => Some(1),
            _ => probes.probed.channels,
        };
        let estimate = wav_estimated_bytes(
            probes.duration_secs,
            channels_per_output,
            manifest::output_sample_rate(&out_codec),
        );
        out_codec.extend(wav_rf64_args(estimate));
    }

    // Start time (DA-08): read the source's BWF bext TimeReference and carry
    // it into WAV outputs, rescaled to the output rate and moved later by
    // exactly what leading-silence trim removed. Every other output has the
    // copied `time_reference` tag removed, so only the export record holds
    // the start.
    let proc = probes.proc.clone();
    let trimmed_samples = probes.leading_trim.as_ref().map_or(0, |trim| trim.samples);
    // Refused whenever trim applies, not only when this recording happens to
    // start with silence, so the same settings behave the same on every file.
    if probes.leading_trim.is_some() {
        refuse_trim_across_sessions(probes.sgmca_source.as_ref())?;
    }
    let start_shifted = trim_moves_start(probes.leading_trim.as_ref());
    let output_rate = manifest::output_sample_rate(&out_codec);
    let mut encoder_args = out_codec;
    if job.format == "sgmca" {
        // One mono Opus stream per output channel, each at the same rate.
        let output_channels = match job.mode.as_str() {
            "stereo" => Some(2),
            "split" => Some(1),
            _ => probes.channels,
        };
        if let Some(channels) = output_channels {
            encoder_args.extend(["-b:a".into(), format!("{}k", SGMCA_KBPS_PER_CHANNEL * channels)]);
        }
    }
    encoder_args.extend(manifest::start_time_metadata_args(
        &job.format,
        probes.probed.timing,
        output_rate,
        trimmed_samples,
    ));
    // Trim and Fade restart the timeline at 0; this keeps the source's own
    // start offset (and adds the recorded trim) where the format carries one.
    encoder_args.extend(crate::ffmpeg::output_start_offset_args(
        job,
        probes.probed.start_offset_micros,
        probes.leading_trim.as_ref(),
    ));

    let mut ffmpeg_args = safe_ffmpeg_input_prelude();
    // A DCR feed's channels are microphones with no speaker positions:
    // FFmpeg must not guess a surround layout (such as 4.0) for them.
    let per_input: Vec<String> = if feed.dcr().is_some() {
        vec!["-guess_layout_max".into(), "0".into()]
    } else {
        Vec::new()
    };
    ffmpeg_args.extend(feed.input_args(&per_input));

    let planned: Vec<PlannedOutput>;
    let map_args: Vec<Vec<String>>;
    let filter_option: Option<&'static str>;
    let filter_chain: Option<String>;
    let mut args = ffmpeg_args;

    match job.mode.as_str() {
        "stereo" => {
            let num_ch = probes
                .channels
                .ok_or("Cannot create stereo mix: unable to determine the input's channel count")?;
            let num_ch = validate_conversion_channels(num_ch)?;
            let weight = 1.0 / num_ch as f64;

            // Use auto-level gains if available, otherwise use manual chan_vols.
            // Gains are only trustworthy when the analysis saw the same channel
            // count we're mixing — its probe can fall back to 1 (or cap at 16)
            // on files this probe still reads fine, and a mismatched vector
            // would boost some channels and silence others.
            let vols: Vec<f64> = match probes.channel_gains {
                Some(ref gains) if job.auto_level && gains.len() == num_ch as usize => {
                    gains.iter().map(|&g| g * weight).collect()
                }
                _ => (0..num_ch)
                    .map(|i| job.chan_vols.get(i as usize).copied().unwrap_or(1.0) * weight)
                    .collect(),
            };

            let pan = stereo_pan_filter(num_ch, &vols);
            let mut all: Vec<String> = std::iter::once(pan).chain(proc).collect();
            // The weight/scale pair makes this a unity-gain SUM of channels:
            // right for turn-taking speakers, but correlated content (every
            // mic hearing the same voice) can exceed full scale by up to
            // 20*log10(N) dB. loudnorm true-peak-limits when normalize is on;
            // otherwise cap peaks ourselves so the encode doesn't hard-clip.
            if !job.normalize {
                all.push(STEREO_PEAK_LIMITER.into());
            }
            planned = reserve_output_paths([(out_dir.join(format!("{}{}", base, ext)), None, None)])?;
            track_planned(cleanup, &planned);
            let dst = planned[0].path.clone();
            let chain = all.join(",");
            let (option, graph, map) = feed.source_filtering(Some(&chain));
            args.extend(feed.source_output_args(Some(&chain)));
            map_args = vec![map];
            args.extend(encoder_args.clone());
            args.extend(["-y".into(), dst.to_string_lossy().to_string()]);
            filter_option = option;
            filter_chain = graph;
        }

        "keep" => {
            validate_keep_channels(&job.format, probes.channels)?;
            planned = reserve_output_paths([(out_dir.join(format!("{}_orig{}", base, ext)), None, None)])?;
            track_planned(cleanup, &planned);
            let dst = planned[0].path.clone();
            let chain = (!proc.is_empty()).then(|| proc.join(","));
            let (option, graph, map) = feed.source_filtering(chain.as_deref());
            args.extend(feed.source_output_args(chain.as_deref()));
            map_args = vec![map];
            filter_option = option;
            filter_chain = graph;
            args.extend(encoder_args.clone());
            args.extend(["-y".into(), dst.to_string_lossy().to_string()]);
        }

        "split" => {
            let num_ch = probes
                .channels
                .ok_or("Cannot split channels: unable to determine the input's channel count")?;
            let num_ch = validate_conversion_channels(num_ch)?;
            let labels = split_labels(num_ch, &job.labels);
            planned = reserve_output_paths(labels.iter().enumerate().map(|(index, label)| {
                (
                    out_dir.join(format!("{}_{}{}", base, label, ext)),
                    Some(index as u32),
                    job.labels.get(index).cloned(),
                )
            }))?;
            track_planned(cleanup, &planned);

            // Use asplit + pan=mono per channel instead of channelsplit:
            // channelsplit requires the actual channel layout (defaulting to
            // stereo), which breaks mono and 4-channel court recordings.
            // Same channel-count guard as the stereo arm: a gains vector from
            // a desynced analysis probe must not be applied per-channel
            let valid_gains = probes.channel_gains.as_ref().filter(|g| g.len() == num_ch as usize);
            let fc = split_filter_complex_from(feed.filter_source(), num_ch, job.auto_level, valid_gains, &proc);
            args.extend(["-filter_complex".into(), fc.clone(), "-y".into()]);
            map_args = (0..planned.len())
                .map(|i| vec!["-map".into(), format!("[op{}]", i)])
                .collect();
            for (output, map) in planned.iter().zip(&map_args) {
                args.extend(map.clone());
                args.extend(encoder_args.clone());
                args.push(output.path.to_string_lossy().to_string());
            }
            filter_option = Some("-filter_complex");
            filter_chain = Some(fc);
        }

        _ => return Err(format!("Unknown mode: {}", job.mode)),
    }

    // A delayed output, in any format, would no longer line up with the
    // recording or a transcript, and its record would say the timeline was
    // kept; refuse it before FFmpeg runs (an SGMCA output is checked again
    // after writing).
    if let Some(filter) = filter_chain.as_deref().and_then(delaying_filter) {
        return Err(format!("{} Nothing was converted.", delay_message(filter)));
    }

    let sgmca_output = if job.format == "sgmca" {
        Some(plan_sgmca_output(
            probes.sgmca_source.as_ref(),
            feed_pcm_wav_frames(feed).or_else(|| probes.source_samples.zip(probes.probed.sample_rate)),
            &probes.probed,
            start_shifted,
            crate::clock::now_utc(),
        )?)
    } else {
        None
    };

    Ok(ConversionPlan {
        planned,
        args,
        filter_option,
        filter_chain,
        encoder_args,
        map_args,
        sgmca_output,
        fade_out: probes.fade_out.clone(),
        leading_trim: probes.leading_trim.clone(),
    })
}

// ── SGMCA output ─────────────────────────────────────────────────────────────

/// The segments an SGMCA output gets and where their start times come from.
#[derive(Debug, Clone, PartialEq)]
struct SgmcaOutputPlan {
    start_source: &'static str,
    note: String,
    /// How far the first sample falls after the whole second written in the
    /// first segment, in milliseconds, when DepoAudio worked the start out
    /// (BWF); None when it was copied from the source's own segment headers
    /// or is the time of conversion.
    start_offset_ms: Option<f64>,
    segments: Vec<crate::sgmca::PlannedSegment>,
    /// How many 48 kHz samples the written output and each of its segments
    /// must hold, from the source's exact length (see `plan_sgmca_output`).
    /// None only after a leading-silence trim that removed samples.
    expected_samples: Option<ExpectedSamples>,
}

/// The samples an SGMCA output must hold: from the source's own pages (an
/// SGMCA source), the data chunk of a PCM WAV feed, or the samples any other
/// source decodes to, counted before converting.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExpectedSamples {
    total: u64,
    /// One per output segment, in order: the matching source session's
    /// samples (or the whole source when the sessions are joined into one).
    segments: Vec<u64>,
    /// 1 when the source is resampled to 48 kHz and its length there is not
    /// a whole number of samples: the output may then hold `total` (the
    /// length rounded down) or one more, never fewer. 0 means exact.
    rounding: u64,
}

/// The exact sample frames and rate of an uncompressed PCM or float WAV
/// (RIFF or RF64), read from its `fmt ` and `data` chunks, which is exactly
/// what FFmpeg decodes from it. None for anything else, and for a WAV whose
/// data length is unknown (0 or 0xFFFFFFFF, as a recorder writes while
/// still recording), not a whole number of frames, or past the end of the
/// file (cut short): FFmpeg's decoded length is then not known exactly.
fn pcm_wav_frames(path: &Path) -> Option<(u64, u32)> {
    use std::io::Read;
    const HEADER_LIMIT: u64 = 1 << 20;
    let mut file = fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    let mut header = Vec::new();
    (&mut file).take(HEADER_LIMIT).read_to_end(&mut header).ok()?;
    let u16_at = |at: usize| -> Option<u16> { Some(u16::from_le_bytes(header.get(at..at + 2)?.try_into().ok()?)) };
    let u32_at = |at: usize| -> Option<u32> { Some(u32::from_le_bytes(header.get(at..at + 4)?.try_into().ok()?)) };
    let u64_at = |at: usize| -> Option<u64> { Some(u64::from_le_bytes(header.get(at..at + 8)?.try_into().ok()?)) };
    let rf64 = match header.get(0..4)? {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return None,
    };
    if header.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut ds64_data: Option<u64> = None;
    let mut format: Option<(u32, u16)> = None;
    let mut at = 12usize;
    loop {
        let id = header.get(at..at + 4)?;
        let size = u32_at(at + 4)?;
        let body = at + 8;
        match id {
            b"ds64" if rf64 => ds64_data = Some(u64_at(body + 8)?),
            b"fmt " => {
                let mut tag = u16_at(body)?;
                if tag == 0xFFFE && size >= 40 {
                    tag = u16_at(body + 24)?;
                }
                // 1: integer PCM, 3: IEEE float. Anything else is coded.
                if !matches!(tag, 1 | 3) {
                    return None;
                }
                let rate = u32_at(body + 4)?;
                let block_align = u16_at(body + 12)?;
                if rate == 0 || block_align == 0 {
                    return None;
                }
                format = Some((rate, block_align));
            }
            b"data" => {
                let (rate, block_align) = format?;
                let data_len = if rf64 && size == u32::MAX {
                    ds64_data?
                } else {
                    u64::from(size)
                };
                let block_align = u64::from(block_align);
                let whole = data_len > 0 && (rf64 || size != u32::MAX) && data_len % block_align == 0;
                let present = (body as u64).checked_add(data_len)? <= file_len;
                return (whole && present).then_some((data_len / block_align, rate));
            }
            _ => {}
        }
        at = body.checked_add(size as usize)?.checked_add(size as usize % 2)?;
    }
}

/// The 48 kHz samples an SGMCA output of a source of `frames` samples at
/// `rate` must hold: its frames, or for another rate the resampled length,
/// rounded down, with one sample of rounding allowed when it is not whole
/// (FFmpeg's resampler gives the length rounded either way).
fn expected_from_source_length(frames: u64, rate: u32) -> ExpectedSamples {
    let scaled = u128::from(frames) * u128::from(crate::sgmca::OPUS_RATE);
    let total = u64::try_from(scaled / u128::from(rate)).unwrap_or(u64::MAX);
    ExpectedSamples {
        total,
        segments: vec![total],
        rounding: u64::from(scaled % u128::from(rate) != 0),
    }
}

/// A filter in `chain` that may delay or move the audio without saying so:
/// one not in `TIMING_SAFE_FILTERS`, or `alimiter` without `latency=1`,
/// which outputs its 5 ms look-ahead as a delay and keeps the length, so
/// every sample after it is late and the last 5 ms of the source are lost.
/// None when every filter in the chain keeps the audio in place.
fn delaying_filter(chain: &str) -> Option<&str> {
    chain.split([',', ';']).find(|filter| {
        let (filter_name, options) = filter_name_and_options(filter);
        if filter_name.is_empty() {
            return false;
        }
        if !TIMING_SAFE_FILTERS.contains(&filter_name) {
            return true;
        }
        let options = options.split('[').next().unwrap_or("").trim();
        match filter_name {
            "alimiter" => !options
                .split(':')
                .any(|option| matches!(option, "latency=1" | "latency=true")),
            "asetpts" => !matches!(options, "N/SR/TB" | "PTS-STARTPTS"),
            "atrim" => !options
                .strip_prefix("start_sample=")
                .is_some_and(|count| !count.is_empty() && count.bytes().all(|byte| byte.is_ascii_digit())),
            _ => false,
        }
    })
}

/// One filter's name and options, without its pad labels (`[sp0]...[op0]`).
fn filter_name_and_options(filter: &str) -> (&str, &str) {
    let mut name = filter.trim();
    while let Some(rest) = name.strip_prefix('[') {
        name = rest.split_once(']').map_or("", |(_, after)| after);
    }
    let (filter_name, options) = name.split_once('=').unwrap_or((name, ""));
    (filter_name.split('[').next().unwrap_or("").trim(), options)
}

/// The check after an SGMCA output is written: no audio dropped, padded,
/// delayed or moved. A filter that delays the audio is refused. When the
/// samples are known exactly (`expected`), the output must hold exactly
/// that many, and each segment exactly its source session's samples.
fn check_sgmca_samples(
    expected: Option<&ExpectedSamples>,
    filter_chain: Option<&str>,
    written: &crate::sgmca::SgmcaInfo,
) -> Result<(), String> {
    if let Some(filter) = filter_chain.and_then(delaying_filter) {
        return Err(delay_message(filter));
    }
    let Some(expected) = expected else {
        return Ok(());
    };
    let allowed = expected.total..=expected.total.saturating_add(expected.rounding);
    if !allowed.contains(&written.samples) {
        let (more, by) = if written.samples > expected.total {
            ("more", written.samples - expected.total)
        } else {
            ("fewer", expected.total - written.samples)
        };
        return Err(format!(
            "It holds {} samples (48 kHz) where the source holds {}: {by} {more}, so audio would have been {} the recording.",
            written.samples,
            expected.total,
            if more == "more" { "added to" } else { "dropped from" }
        ));
    }
    if written.segments.len() != expected.segments.len() {
        return Err(format!(
            "It has {} segments where {} were planned, one per recording session of the source.",
            written.segments.len(),
            expected.segments.len()
        ));
    }
    for (index, (segment, &want)) in written.segments.iter().zip(&expected.segments).enumerate() {
        if !(want..=want.saturating_add(expected.rounding)).contains(&segment.samples) {
            return Err(format!(
                "Its segment {} holds {} samples (48 kHz) where the matching source session holds {want}, so audio would have moved from one recording session to another.",
                index + 1,
                segment.samples
            ));
        }
    }
    Ok(())
}

fn delay_message(filter: &str) -> String {
    let (name, _) = filter_name_and_options(filter);
    if name == "alimiter" {
        format!(
            "The processing chain includes {name} without latency compensation, which delays every sample by its look-ahead and drops the same length from the end, so the output would no longer line up with the recording or its transcript."
        )
    } else {
        format!(
            "The processing chain includes {name}, a filter DepoAudio has not checked for delaying or moving the audio, so the output might no longer line up with the recording or its transcript."
        )
    }
}

/// Where a BWF source's audio is placed in an SGMCA output.
#[derive(Debug, Clone, PartialEq)]
struct BwfStart {
    /// The start text: the origination date plus the time reference, at the
    /// whole second at or before the source's first sample.
    text: String,
    /// How far the source's first sample falls after that whole second, in
    /// milliseconds (0 when the time reference falls on a whole second).
    /// Every source sample is kept; this is recorded, never cut.
    offset_ms: f64,
}

/// The start of a BWF source as an SGMCA start text, with this computer's
/// UTC offset for that date (BWF stores none). The segment header holds
/// whole seconds only, so when the time reference falls between two seconds
/// the text is the whole second at or before the first sample, and the
/// fraction is returned so the export record can say so. No audio is ever
/// left out to make the time round.
fn bwf_start_text(timing: manifest::SourceTiming, date: &str) -> Option<BwfStart> {
    let year: i32 = date.get(0..4)?.parse().ok()?;
    let month = time::Month::try_from(date.get(5..7)?.parse::<u8>().ok()?).ok()?;
    let day: u8 = date.get(8..10)?.parse().ok()?;
    let date = time::Date::from_calendar_date(year, month, day).ok()?;
    let rate = u64::from(timing.sample_rate);
    if rate == 0 {
        return None;
    }
    let whole = timing.time_reference_samples / rate;
    let past = timing.time_reference_samples % rate;
    let local = time::PrimitiveDateTime::new(date, time::Time::MIDNIGHT)
        .checked_add(time::Duration::seconds(i64::try_from(whole).ok()?))?;
    // The offset in force at that wall-clock time: first guess from the
    // instant as if it were UTC, then once more from the instant it names.
    let guess = crate::clock::local_offset_at(local.assume_utc()).unwrap_or(time::UtcOffset::UTC);
    let offset = crate::clock::local_offset_at(local.assume_offset(guess)).unwrap_or(guess);
    let text = crate::sgmca::format_start_text(local, i32::from(offset.whole_minutes()))?;
    Some(BwfStart {
        text,
        offset_ms: crate::sgmca::offset_ms(past, rate),
    })
}

/// Leading-silence trim removes audio from the start of the first session.
/// On an SGMCA source with more than one recording session that moves every
/// session's place in the output and its start time, which DepoAudio does
/// not yet rewrite (an SGMCA output would also lose its session boundaries),
/// so it is refused before anything is converted.
fn refuse_trim_across_sessions(source: Option<&crate::sgmca::SgmcaInfo>) -> Result<(), String> {
    let sessions = source.map_or(0, |info| {
        info.segments.iter().filter(|segment| segment.samples > 0).count()
    });
    if sessions > 1 {
        return Err(format!(
            "Trim Leading Silence cannot be used on this file: it holds {sessions} recording sessions, and trim removes audio from the start of the first one, which would move where each session starts in the output, and its start time (an SGMCA output would also lose the session boundaries). Turn off Trim Leading Silence to convert it. Nothing was converted."
        ));
    }
    Ok(())
}

const CONVERSION_TIME_EXPLAINED: &str = "It is not when the recording was made. CATalyst lines Direct Audio up with a transcript by matching segment start times with stroke times, so this file will not line up with a transcript until a correct start is known.";

/// Where an SGMCA output's segment start times come from, in order of
/// preference: the source's own SGMCA segment headers (one output segment
/// per source session, each starting where that session starts in the
/// joined audio), the source's BWF origination date and bext time
/// reference, or the time of conversion. Leading-silence trim moves the
/// first sample later; when it removed any samples, only the conversion time
/// is used (the recorded trim is in the export record).
fn plan_sgmca_output(
    source: Option<&crate::sgmca::SgmcaInfo>,
    source_length: Option<(u64, u32)>,
    probed: &manifest::ProbedSource,
    shifted: bool,
    now: time::OffsetDateTime,
) -> Result<SgmcaOutputPlan, String> {
    let conversion_time = |reason: &str| -> Result<SgmcaOutputPlan, String> {
        let text = crate::clock::local_start_text(now)
            .ok_or("The time of conversion cannot be written as an SGMCA start time.")?;
        Ok(SgmcaOutputPlan {
            start_source: "conversion-time",
            note: format!("{reason} The segment start is the time of this conversion. {CONVERSION_TIME_EXPLAINED}"),
            start_offset_ms: None,
            segments: vec![crate::sgmca::PlannedSegment {
                start_text: text,
                start_sample: 0,
            }],
            expected_samples: None,
        })
    };
    let mut plan = plan_sgmca_start(source, probed, shifted, conversion_time)?;
    // An SGMCA source's samples are known exactly from its pages: unless a
    // filter moved the start, the output must hold exactly those, each
    // segment its own session's.
    if let Some(info) = source.filter(|_| !shifted) {
        let kept: Vec<u64> = info
            .segments
            .iter()
            .map(|segment| segment.samples)
            .filter(|&samples| samples > 0)
            .collect();
        plan.expected_samples = Some(ExpectedSamples {
            total: info.samples,
            segments: if plan.segments.len() == kept.len() {
                kept
            } else {
                vec![info.samples]
            },
            rounding: 0,
        });
    } else if source.is_none() && !shifted {
        // Any other source's length is exact too (a PCM WAV's data chunk, or
        // the samples it decodes to, counted before converting), so an
        // encoder or muxer that dropped the last partial Opus frame, or
        // padded the end, fails here on any FFmpeg version.
        let (frames, rate) = source_length.ok_or(SOURCE_LENGTH_UNKNOWN_MESSAGE)?;
        plan.expected_samples = Some(expected_from_source_length(frames, rate));
    }
    Ok(plan)
}

fn plan_sgmca_start(
    source: Option<&crate::sgmca::SgmcaInfo>,
    probed: &manifest::ProbedSource,
    shifted: bool,
    conversion_time: impl Fn(&str) -> Result<SgmcaOutputPlan, String>,
) -> Result<SgmcaOutputPlan, String> {
    if shifted {
        return conversion_time(
            "Leading-silence trim removed audio from the start (the export record says how many samples), so the source start time does not describe this output.",
        );
    }
    if let Some(info) = source.filter(|info| info.container == crate::sgmca::Container::DirectAudio) {
        let kept: Vec<&crate::sgmca::SegmentInfo> =
            info.segments.iter().filter(|segment| segment.samples > 0).collect();
        // A start text CATalyst could not parse would leave the output
        // unplaceable against a transcript, so it is never copied.
        let unreadable: Vec<String> = kept
            .iter()
            .filter(|segment| segment.start.as_ref().is_none_or(|start| start.local.is_none()))
            .map(|segment| segment.number.to_string())
            .collect();
        if !unreadable.is_empty() {
            let (which, verb) = if unreadable.len() == 1 {
                (format!("source segment {}", unreadable[0]), "is")
            } else {
                (format!("source segments {}", unreadable.join(", ")), "are")
            };
            return conversion_time(&format!(
                "The start text of {which} {verb} not in the YYYY-MM-DDTHH:MM:SS±hhmm form, so it cannot be copied into a header CATalyst reads. The recording sessions are joined into one segment."
            ));
        }
        let segments: Vec<crate::sgmca::PlannedSegment> = kept
            .iter()
            .map(|segment| crate::sgmca::PlannedSegment {
                start_text: segment
                    .start
                    .as_ref()
                    .map(|start| start.text.clone())
                    .unwrap_or_default(),
                start_sample: segment.start_sample,
            })
            .collect();
        if segments.is_empty() {
            return Err("The SGMCA source holds no audio.".into());
        }
        let mut note = if segments.len() == 1 {
            "The segment keeps the start time from the source's segment header.".to_string()
        } else {
            format!(
                "Each of the {} recording sessions keeps its start time from the source's segment headers, and starts at the same point of the audio as in the source.",
                segments.len()
            )
        };
        if info.growing {
            note.push_str(" The source looked still recording or cut short, so only its complete audio was converted.");
        }
        return Ok(SgmcaOutputPlan {
            start_source: "sgmca-segment-start",
            note,
            // The same samples follow the same text as in the source, so
            // nothing is known or changed beyond what its headers say.
            start_offset_ms: None,
            segments,
            expected_samples: None,
        });
    }
    // Only a BWF bext time reference is dated by the WAV's own date tag; an
    // FTR recording's start (TLOC, TUTC or file name) is kept in the export
    // record's source.trm and is not written into segment headers.
    let bext_timing = probed
        .timing
        .filter(|timing| timing.source == manifest::BEXT_TIMING_SOURCE);
    if let (Some(timing), Some(date)) = (bext_timing, probed.origination_date.as_deref()) {
        if let Some(start) = bwf_start_text(timing, date) {
            let offset = if start.offset_ms == 0.0 {
                "The time reference falls on a whole second.".to_string()
            } else {
                format!(
                    "{} Every sample of the source is kept.",
                    crate::sgmca::whole_second_note(start.offset_ms)
                )
            };
            return Ok(SgmcaOutputPlan {
                start_source: "bwf-bext",
                note: format!(
                    "The start is the source's date plus its BWF bext time reference. {offset} The date is the WAV's date tag, which FFmpeg reports for the bext origination date (a RIFF INFO creation date has the same name). BWF stores no UTC offset, so the offset written is this computer's for that date."
                ),
                start_offset_ms: Some(start.offset_ms),
                segments: vec![crate::sgmca::PlannedSegment {
                    start_text: start.text,
                    start_sample: 0,
                }],
                expected_samples: None,
            });
        }
    }
    if bext_timing.is_some() {
        return conversion_time(
            "The source's BWF time reference has no usable origination date, so the start date is not known.",
        );
    }
    if probed.timing.is_some() {
        return conversion_time(
            "The recording's own start time is kept in the export record's source, not written into SGMCA segment headers.",
        );
    }
    conversion_time("The source carries no start time DepoAudio can read.")
}

/// Wrap the bare Ogg Opus stream FFmpeg wrote at `path` in the Direct Audio
/// headers, in a temporary file next to it that then replaces it, and read
/// the result back. Any failure leaves the output for `OutputCleanup` to
/// remove.
fn write_sgmca_output(
    path: &Path,
    plan: &SgmcaOutputPlan,
    filter_chain: Option<&str>,
    cancelled: crate::sgmca::CancelCheck<'_>,
) -> Result<manifest::SgmcaOutputRecord, String> {
    let name = path.file_name().and_then(|name| name.to_str()).unwrap_or("output");
    let temp = path.with_file_name(format!(
        ".{name}.{}.part",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    ));
    let _guard = TempFile::new(temp.clone());
    crate::sgmca::write_direct_audio(path, &temp, &plan.segments, cancelled)?;
    fs::rename(&temp, path).map_err(|error| format!("Could not finish the SGMCA output: {error}"))?;
    let expected: Vec<String> = plan.segments.iter().map(|segment| segment.start_text.clone()).collect();
    let info = crate::sgmca::verify_written(path, Some(&expected), cancelled)?;
    check_sgmca_samples(plan.expected_samples.as_ref(), filter_chain, &info)?;
    Ok(manifest::SgmcaOutputRecord {
        file_version: 1,
        guid_present: info.guid_present,
        start_source: plan.start_source,
        start_note: plan.note.clone(),
        start_offset_ms: plan.start_offset_ms,
        segments: manifest::sgmca_segment_records(&info),
    })
}

/// The finished outputs with their sizes. An empty output fails the whole
/// conversion; the caller's `OutputCleanup` then removes every sibling.
fn collect_outputs(planned: &[PlannedOutput]) -> Result<Vec<OutputFile>, String> {
    let files: Vec<OutputFile> = planned
        .iter()
        .map(|output| {
            let p = &output.path;
            let size = fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            OutputFile {
                name: basename(&p.to_string_lossy()),
                path: p.to_string_lossy().to_string(),
                size,
            }
        })
        .collect();
    if let Some(empty) = files.iter().find(|f| f.size == 0) {
        return Err(format!("Output file is empty: {}", empty.name));
    }
    Ok(files)
}

/// The export-record facts for one finished output of `plan`.
fn output_facts<'a>(
    job: &'a ConvertJob,
    plan: &ConversionPlan,
    index: usize,
    file: &OutputFile,
    sha256: String,
    size_bytes: u64,
    wav_header: Option<&'static str>,
) -> OutputFacts<'a> {
    OutputFacts {
        file_name: file.name.clone(),
        sha256,
        size_bytes,
        format: &job.format,
        mode: &job.mode,
        filter_option: plan.filter_option,
        filter_chain: plan.filter_chain.clone(),
        encoder_args: plan.encoder_args.clone(),
        map_args: plan.map_args[index].clone(),
        fade_out: plan.fade_out.clone(),
        source_channel_index: plan.planned[index].channel_index,
        label: plan.planned[index].label.clone(),
        wav_header,
        leading_trim: plan.leading_trim.clone(),
        sgmca: None,
    }
}

/// Everything after FFmpeg exits, shared by `do_convert_inner` and the tests:
/// check that no output is empty, fingerprint each output, write its export
/// record (DA-08) and only then commit. An error or a cancel at any point
/// drops `cleanup` uncommitted, which removes every output and placeholder,
/// so no file is delivered without its record. Blocking (file hashing): the
/// app runs it on a blocking thread.
fn finish_outputs(
    job: &ConvertJob,
    plan: &ConversionPlan,
    input: &ManifestInput<'_>,
    mut cleanup: OutputCleanup,
    cancel: &ConversionCancel,
) -> Result<Vec<OutputFile>, String> {
    cancel.check()?;
    // FFmpeg wrote each SGMCA output as a bare Ogg Opus stream: give it the
    // Direct Audio headers, then read it back. Every SGMCA output must really
    // be Direct Audio, with one uncoupled mono stream per channel and the
    // planned segment start times, or it is removed.
    let mut sgmca_records: Vec<Option<manifest::SgmcaOutputRecord>> = vec![None; plan.planned.len()];
    if let Some(sgmca) = &plan.sgmca_output {
        collect_outputs(&plan.planned)?;
        for (output, record) in plan.planned.iter().zip(sgmca_records.iter_mut()) {
            let cancelled = || cancel.cancelled();
            *record = Some(
                write_sgmca_output(&output.path, sgmca, plan.filter_chain.as_deref(), Some(&cancelled)).map_err(
                    |error| {
                        cancel.check().err().unwrap_or_else(|| {
                            format!(
                                "{} failed the SGMCA format check, so the output was removed: {error}",
                                basename(&output.path.to_string_lossy())
                            )
                        })
                    },
                )?,
            );
        }
    }
    let files = collect_outputs(&plan.planned)?;
    for (index, ((output, file), sgmca)) in plan.planned.iter().zip(&files).zip(sgmca_records).enumerate() {
        let hashed = manifest::sha256_file(&output.path, &|| cancel.cancelled());
        cancel.check()?;
        let (sha256, size_bytes) = hashed?;
        // Which header the WAV muxer wrote, read from the file itself.
        let wav_header = if job.format == "wav" {
            Some(manifest::wav_header_kind(&output.path)?)
        } else {
            None
        };
        let mut facts = output_facts(job, plan, index, file, sha256, size_bytes, wav_header);
        facts.sgmca = sgmca;
        let record = manifest::build_manifest(input, facts);
        manifest::write_manifest(&output.manifest, &record).map_err(|error| {
            format!(
                "The audio was converted, but its export record could not be saved, so the output was removed: {error}"
            )
        })?;
    }
    cancel.check()?;
    cleanup.commit();
    Ok(files)
}

async fn do_convert_inner(
    app: &AppHandle,
    job: &ConvertJob,
    feed: &AudioFeed,
    cancel: &ConversionCancel,
    source: FileRecord,
) -> Result<Vec<OutputFile>, InnerError> {
    cancel.check()?;
    let sgmca_source = feed.sgmca().cloned();
    let is_cancelled = || cancel.cancelled();

    let mut channel_gains: Option<Vec<f64>> = None;

    if job.auto_level {
        // Phase: Analyzing
        let _ = app.emit(
            "convert:progress",
            ProgressEvent {
                id: job.id.clone(),
                seconds: 0.0,
                phase: Some("analyzing".into()),
                total: None,
            },
        );

        // Analyze only representative per-channel loudness. Auto-leveling
        // stays on a bounded, non-learned pass and does not predecode the full
        // recording to PCM.
        let analysis_ctx = crate::analysis::ScanCtx::silent(
            app.clone(),
            job.src_path.clone(),
            cancel.epoch.clone(),
            cancel.generation,
        );
        let gain_result = crate::analysis::analyze_feed_channel_gains(app, feed, Some(&analysis_ctx)).await;
        cancel.check()?;
        channel_gains =
            Some(gain_result.map_err(|error| InnerError::Decode(format!("Auto-level analysis failed: {error}")))?);
    }

    let effective_feed = feed;

    // Source duration for a determinate encode progress bar. The output is
    // roughly the input's length (trim shortens it slightly), so the UI caps
    // at 99% until done rather than pretending exact knowledge.
    let total_secs = probe_duration_cancellable(app, effective_feed, Some(&is_cancelled)).await;
    cancel.check()?;

    let probed = probe_manifest_source_cancellable(app, effective_feed, Some(&is_cancelled)).await;
    cancel.check()?;

    // Stereo injects per-channel gains in its pan filter and Split injects
    // them after isolating each channel. Keep Original rejects auto-leveling
    // before work starts because one shared gain would not balance channels.
    // Trim silence counts at the converted stream's rate from the probe.
    let built = build_proc_filters_with_gain(
        app,
        job,
        effective_feed,
        None,
        total_secs,
        probed.sample_rate,
        Some(&is_cancelled),
    )
    .await;
    cancel.check()?;
    let built = built?;

    // Mix to Stereo and Split need the channel count; Keep Original does not,
    // except for an SGMCA output, whose bitrate is set per channel, and a
    // lossy format that cannot keep more than two channels.
    let channels =
        if matches!(job.mode.as_str(), "stereo" | "split") || job.format == "sgmca" || keep_needs_channel_count(job) {
            let channels = probe_channels_cancellable(app, effective_feed, Some(&is_cancelled)).await;
            cancel.check()?;
            channels
        } else {
            None
        };

    let source_samples = if needs_source_sample_count(
        job,
        effective_feed,
        sgmca_source.is_some(),
        trim_moves_start(built.leading_trim.as_ref()),
    ) {
        let counted = crate::ffmpeg::measure_source_samples_cancellable(
            app,
            effective_feed,
            u64::from(job.ffmpeg_timeout),
            Some(&is_cancelled),
        )
        .await;
        cancel.check()?;
        Some(counted.ok_or_else(|| SOURCE_LENGTH_UNKNOWN_MESSAGE.to_string())?)
    } else {
        None
    };

    let probes = ConversionProbes {
        channels,
        channel_gains,
        proc: built.filters,
        fade_out: built.fade_out,
        leading_trim: built.leading_trim,
        probed,
        duration_secs: total_secs,
        sgmca_source,
        source_samples,
    };
    let mut output_cleanup = OutputCleanup::default();
    let plan = plan_conversion(job, effective_feed, &probes, &mut output_cleanup)?;
    run_ffmpeg_with_timeout(
        app,
        plan.args.clone(),
        &job.id,
        job.ffmpeg_timeout as u64,
        total_secs,
        Some(&is_cancelled),
    )
    .await
    .map_err(|error| {
        if error == CONVERSION_CANCELLED_MESSAGE {
            InnerError::Other(error)
        } else {
            InnerError::Decode(error)
        }
    })?;
    cancel.check()?;

    // Outputs, fingerprints and export records (DA-08), then commit; the same
    // function the synthetic court-format tests call. `output_cleanup` moves
    // in, so any failure there removes every output.
    let app_version = app.package_info().version.to_string();
    let created_at = crate::clock::rfc3339_seconds_z(&crate::clock::now_utc());
    let record_job = job.clone();
    let probed = probes.probed;
    let record_cancel = cancel.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let input = ManifestInput {
            app_version,
            created_at,
            source,
            probed: &probed,
            labels: &record_job.labels,
        };
        finish_outputs(&record_job, &plan, &input, output_cleanup, &record_cancel)
    })
    .await
    .map_err(|error| format!("Could not save the export records: {error}"))?
    .map_err(InnerError::Other)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::helpers::prepare_audio_feed_with_cancel;

    fn valid_job() -> ConvertJob {
        serde_json::from_value(serde_json::json!({
            "id": "test",
            "cancelGeneration": 1,
            "srcPath": "/input.wav",
            "outDir": "",
            "mode": "stereo",
            "format": "wav",
            "rate": "48000",
            "labels": [],
            "chanVols": [1.0],
            "normalize": false,
            "trim": false,
            "fade": false,
            "fadeDur": 0.5,
            "hpf": false,
            "caseName": null
        }))
        .unwrap()
    }

    struct ConversionTestDir(PathBuf);

    impl ConversionTestDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "depoaudio_conversion_test_{}",
                uuid::Uuid::new_v4().to_string().replace('-', "")
            ));
            fs::create_dir(&path).expect("create conversion test directory");
            Self(path)
        }
    }

    impl Drop for ConversionTestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    // Golden-master tests: these strings are the audio-processing contract.
    // A change here changes what users' converted files sound like.

    #[test]
    fn stereo_pan_sums_all_channels_into_both_sides() {
        // 4-channel court recording, equal vols of 1/4 each
        let vols = vec![0.25, 0.25, 0.25, 0.25];
        assert_eq!(
            stereo_pan_filter(4, &vols),
            "aformat=sample_fmts=fltp,pan=stereo|c0=0.2500*c0+0.2500*c1+0.2500*c2+0.2500*c3|c1=0.2500*c0+0.2500*c1+0.2500*c2+0.2500*c3,volume=4.0"
        );
    }

    #[test]
    fn stereo_pan_missing_vols_fall_back_to_equal_weight() {
        assert_eq!(
            stereo_pan_filter(2, &[]),
            "aformat=sample_fmts=fltp,pan=stereo|c0=0.5000*c0+0.5000*c1|c1=0.5000*c0+0.5000*c1,volume=2.0"
        );
    }

    #[test]
    fn stereo_pan_carries_per_channel_gains() {
        // Auto-level: gains 2.0 and 0.5 pre-multiplied by the 1/2 weight
        assert_eq!(
            stereo_pan_filter(2, &[1.0, 0.25]),
            "aformat=sample_fmts=fltp,pan=stereo|c0=1.0000*c0+0.2500*c1|c1=1.0000*c0+0.2500*c1,volume=2.0"
        );
    }

    #[test]
    fn split_labels_sanitize_and_fall_back() {
        let labels = vec!["Judge Smith".into(), "".into(), "a/b".into()];
        assert_eq!(split_labels(4, &labels), vec!["Judge_Smith", "ch2", "a_b", "ch4"]);
    }

    #[test]
    fn split_filter_uses_asplit_and_pan_mono_per_channel() {
        // channelsplit would assume a stereo layout; asplit+pan works for any
        // channel count — the fix that made 4-channel court files split right
        assert_eq!(
            split_filter_complex(2, false, None, &[]),
            "[0:a:0]asplit=2[sp0][sp1];[sp0]pan=mono|c0=c0[op0];[sp1]pan=mono|c0=c1[op1]"
        );
    }

    #[test]
    fn split_filter_appends_shared_proc_chain_per_channel() {
        let proc = vec!["highpass=f=80".to_string(), "loudnorm=I=-16:TP=-1.5:LRA=11".to_string()];
        assert_eq!(
            split_filter_complex(2, false, None, &proc),
            "[0:a:0]asplit=2[sp0][sp1];[sp0]pan=mono|c0=c0,highpass=f=80,loudnorm=I=-16:TP=-1.5:LRA=11[op0];[sp1]pan=mono|c0=c1,highpass=f=80,loudnorm=I=-16:TP=-1.5:LRA=11[op1]"
        );
    }

    #[test]
    fn split_filter_injects_each_channels_own_gain() {
        let gains = vec![2.0, 1.0, 0.5];
        assert_eq!(
            split_filter_complex(3, true, Some(&gains), &[]),
            "[0:a:0]asplit=3[sp0][sp1][sp2];[sp0]pan=mono|c0=c0,volume=2.0000[op0];[sp1]pan=mono|c0=c1[op1];[sp2]pan=mono|c0=c2,volume=0.5000[op2]"
        );
    }

    #[test]
    fn split_filter_ignores_gains_when_auto_level_off() {
        let gains = vec![2.0, 0.5];
        assert_eq!(
            split_filter_complex(2, false, Some(&gains), &[]),
            "[0:a:0]asplit=2[sp0][sp1];[sp0]pan=mono|c0=c0[op0];[sp1]pan=mono|c0=c1[op1]"
        );
    }

    #[test]
    fn failed_split_reservation_removes_earlier_placeholders() {
        let dir = ConversionTestDir::new();
        let first = dir.0.join("first.wav");
        let invalid = dir.0.join("missing-parent").join("second.wav");

        let result = reserve_output_paths([(first.clone(), Some(0), None), (invalid, Some(1), None)]);

        assert!(result.is_err());
        assert!(!first.exists(), "the first reservation must be rolled back");
        assert!(
            !crate::manifest::manifest_path_for(&first).exists(),
            "the first export record reservation must be rolled back"
        );
    }

    #[test]
    fn every_output_reserves_its_export_record_next_to_it() {
        let dir = ConversionTestDir::new();
        let planned = reserve_output_paths([
            (dir.0.join("session_Attorney.wav"), Some(0), Some("Attorney".into())),
            (dir.0.join("session_Witness.wav"), Some(1), Some("Witness".into())),
        ])
        .unwrap();
        assert_eq!(planned.len(), 2);
        for output in &planned {
            assert!(output.path.exists());
            assert_eq!(output.manifest, crate::manifest::manifest_path_for(&output.path));
            assert_eq!(fs::metadata(&output.manifest).unwrap().len(), 0);
        }
        assert_eq!(planned[1].channel_index, Some(1));
        assert_eq!(planned[1].label.as_deref(), Some("Witness"));

        // Cleanup of an uncommitted conversion removes outputs and records.
        {
            let mut cleanup = OutputCleanup::default();
            track_planned(&mut cleanup, &planned);
        }
        for output in &planned {
            assert!(!output.path.exists());
            assert!(!output.manifest.exists());
        }
    }

    #[test]
    fn every_learned_processing_flag_is_rejected_with_the_release_message() {
        for field in ["denoise", "enhance", "dereverb"] {
            let mut job = valid_job();
            match field {
                "denoise" => job.denoise = true,
                "enhance" => job.enhance = true,
                "dereverb" => job.dereverb = true,
                _ => unreachable!(),
            }

            let error = validate_convert_request(&job).unwrap_err();
            assert!(error.contains(&format!("DepoAudio v{}", env!("CARGO_PKG_VERSION"))));
            assert!(error.contains("not included"));
            assert!(error.contains("Noise Removal"));
            assert!(error.contains("Enhance Clarity"));
            assert!(error.contains("Reduce Room Echo"));
        }
    }

    #[test]
    fn learned_processing_rejection_precedes_other_request_validation() {
        let mut job = valid_job();
        job.denoise = true;
        job.mode = "unsupported".into();
        job.src_path = "a path that must never be inspected".into();

        let error = validate_convert_request(&job).unwrap_err();
        assert!(error.contains(&format!("DepoAudio v{}", env!("CARGO_PKG_VERSION"))));
        assert!(!error.contains("Unsupported conversion mode"));
    }

    #[test]
    fn conversion_channel_guard_accepts_registry_limit_and_rejects_above_it() {
        assert_eq!(validate_conversion_channels(16).unwrap(), 16);
        assert!(validate_conversion_channels(0).unwrap_err().contains("zero"));
        let error = validate_conversion_channels(17).unwrap_err();
        assert!(error.contains("17"));
        assert!(error.contains("at most 16"));
    }

    #[test]
    fn conversion_request_rejects_unknown_modes_and_formats_early() {
        let mut job = valid_job();
        job.mode = "surround".into();
        assert!(validate_convert_request(&job).unwrap_err().contains("mode"));

        job.mode = "stereo".into();
        job.format = "exe".into();
        assert!(validate_convert_request(&job).unwrap_err().contains("format"));
    }

    #[test]
    fn keep_original_to_a_lossy_format_needs_a_mono_or_stereo_source() {
        for format in ["mp3", "m4a", "opus"] {
            assert!(validate_keep_channels(format, Some(1)).is_ok());
            assert!(validate_keep_channels(format, Some(2)).is_ok());
            let error = validate_keep_channels(format, Some(4)).unwrap_err();
            assert!(error.contains("4-channel"), "{error}");
            assert!(validate_keep_channels(format, None)
                .unwrap_err()
                .contains("channel count"));
            let mut job = valid_job();
            job.mode = "keep".into();
            job.format = format.into();
            assert!(keep_needs_channel_count(&job));
            job.mode = "split".into();
            assert!(!keep_needs_channel_count(&job));
        }
        for format in ["wav", "flac", "sgmca"] {
            assert!(validate_keep_channels(format, Some(16)).is_ok());
            assert!(validate_keep_channels(format, None).is_ok());
        }
    }

    #[test]
    fn conversion_request_rejects_auto_level_in_keep_mode() {
        let mut job = valid_job();
        job.mode = "keep".into();
        job.auto_level = true;

        let error = validate_convert_request(&job).unwrap_err();
        assert!(error.contains("Auto-leveling"));
        assert!(error.contains("Keep Original"));
    }

    #[test]
    fn conversion_request_rejects_nonfinite_or_out_of_range_settings() {
        let mut job = valid_job();
        job.max_file_size_gb = f64::NAN;
        assert!(validate_convert_request(&job).is_err());

        job = valid_job();
        job.hpf_cutoff = 501.0;
        assert!(validate_convert_request(&job).unwrap_err().contains("High-pass"));

        job = valid_job();
        job.normalize_lufs = -71.0;
        assert!(validate_convert_request(&job).unwrap_err().contains("loudness"));

        job = valid_job();
        job.normalize_tp = 0.1;
        assert!(validate_convert_request(&job).unwrap_err().contains("true peak"));

        job = valid_job();
        job.silence_thresh = f64::INFINITY;
        assert!(validate_convert_request(&job).unwrap_err().contains("Silence"));

        job = valid_job();
        job.chan_vols = vec![f64::NAN];
        assert!(validate_convert_request(&job).unwrap_err().contains("Channel volumes"));
    }

    #[test]
    fn non_learned_declip_and_auto_level_remain_valid() {
        let mut job = valid_job();
        job.declip = true;
        job.auto_level = true;
        assert!(validate_convert_request(&job).is_ok());
    }

    #[test]
    fn conversion_cancel_generation_invalidates_stale_jobs() {
        let epoch = Arc::new(AtomicU64::new(7));
        let cancel = ConversionCancel::new(epoch.clone(), 7);
        assert!(cancel.check().is_ok());
        epoch.store(8, Ordering::Release);
        assert_eq!(cancel.check().unwrap_err(), CONVERSION_CANCELLED_MESSAGE);
    }

    #[test]
    fn uncommitted_output_cleanup_removes_reserved_placeholders() {
        let dir = ConversionTestDir::new();
        let output = dir.0.join("cancelled.wav");
        fs::write(&output, b"partial").unwrap();
        {
            let mut cleanup = OutputCleanup::default();
            cleanup.track(output.clone());
        }
        assert!(!output.exists());
    }

    #[test]
    fn committed_output_cleanup_preserves_complete_outputs() {
        let dir = ConversionTestDir::new();
        let output = dir.0.join("complete.wav");
        fs::write(&output, b"complete").unwrap();
        {
            let mut cleanup = OutputCleanup::default();
            cleanup.track(output.clone());
            cleanup.commit();
        }
        assert!(output.exists());
    }

    // ── Synthetic court-format conversions (FFmpeg on PATH) ─────────────────
    //
    // These call the conversion's own request checks, source preparation and
    // fingerprint, ffprobe arguments, filter builder, plan (outputs and FFmpeg
    // arguments) and `finish_outputs` (output checks, hashes, export records,
    // commit) with FFmpeg and ffprobe from PATH (CI installs them from apt).
    // What `convert_with_system_ffmpeg` does NOT share with `do_convert_inner`
    // is the async part, which needs the Tauri runtime: the order of the
    // probes and their cancel checks, the auto-level analysis (a test passes
    // the gains it wants instead), the sidecar launch and progress events. The
    // harness mirrors that part, so a change there needs the same change here.
    // The audio is SYNTHETIC: lavfi sine tones, no speech, and a made-up vendor
    // prefix. No byte of a real Stenograph or DigitalCAT file is copied. Each
    // test returns early, with a message, when the tools are missing, unless
    // DEPOAUDIO_REQUIRE_FFMPEG_TESTS=1 (set in CI), when it fails instead.

    /// FFmpeg and ffprobe are on PATH and FFmpeg can encode multichannel Opus
    /// (the shipped LGPL builds use libopus).
    fn court_fixture_tools_available() -> bool {
        let tools = std::process::Command::new("ffmpeg").arg("-version").output().is_ok()
            && std::process::Command::new("ffprobe").arg("-version").output().is_ok();
        let libopus = tools
            && std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-encoders"])
                .output()
                .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("libopus"));
        if !libopus {
            assert!(
                std::env::var_os("DEPOAUDIO_REQUIRE_FFMPEG_TESTS").is_none_or(|value| value != "1"),
                "DEPOAUDIO_REQUIRE_FFMPEG_TESTS=1, but ffmpeg/ffprobe with libopus is not on PATH"
            );
            eprintln!("skipping: ffmpeg/ffprobe with libopus not on PATH");
        }
        libopus
    }

    /// Levels of the four synthetic channels, about 6 dB apart, so a test can
    /// tell which source channel ended up in which output.
    const SYNTHETIC_CHANNEL_VOLUMES: [f64; 4] = [0.5, 0.25, 0.125, 0.0625];
    const SYNTHETIC_SECONDS: f64 = 2.0;
    /// Samples per frame for the synthetic sine sources: 20 ms at
    /// `sample_rate`, so every synthetic length is a whole number of source
    /// frames and of Opus frames (FFmpeg's default of 1024 divides neither
    /// 2 s at 48 kHz nor at 16 kHz).
    const fn synthetic_frame(sample_rate: u32) -> u32 {
        sample_rate / 50
    }

    /// Made-up bytes that are not a Direct Audio header (nor an Ogg page).
    fn synthetic_vendor_prefix() -> Vec<u8> {
        let mut prefix = b"DEPOAUDIO SYNTHETIC TEST PREFIX - NOT A REAL SGMCA HEADER\n".to_vec();
        prefix.resize(1024, 0x5a);
        assert!(!prefix.windows(4).any(|window| window == b"OggS"));
        prefix
    }

    /// `helpers::finish_prepared_feed` with FFmpeg from PATH: run a DCR
    /// feed's channel merge, then clear the guessed speaker mask.
    fn finish_with_system_ffmpeg(mut prepared: AudioFeed) -> Result<AudioFeed, String> {
        if let Some(merge) = prepared.take_merge() {
            let output = std::process::Command::new("ffmpeg").args(&merge.args).output().unwrap();
            if !output.status.success() {
                let codec = prepared.dcr().map(|info| info.codec.clone()).unwrap_or_default();
                return Err(crate::helpers::dcr_merge_error(&codec, &output.stderr));
            }
            crate::dcr::clear_channel_mask(prepared.path())?;
        }
        Ok(prepared)
    }

    fn run_ffmpeg(args: &[String]) {
        let output = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Four mono tones joined into one 4-channel quad stream, at the levels in
    /// `SYNTHETIC_CHANNEL_VOLUMES`, written with `encoder` to `path`. The tones
    /// are generated at `sample_rate`, so an encoder at that rate gets exactly
    /// `SYNTHETIC_SECONDS` of samples with no resampler in between.
    ///
    /// The tones are sources inside the one filter graph, not separate
    /// `-f lavfi` inputs: FFmpeg 7 and later demux each input in its own
    /// thread, and `join` ends at the first input's end, so with separate
    /// inputs it sometimes dropped the last frame of the others (FFmpeg
    /// 8.0.1: 95,040 of 96,000 samples in 2 of 6 runs, so the fixture, not
    /// the conversion, was short; 6.1 always kept them). Inside one graph
    /// `join` kept all 96,000 in every run on both versions.
    fn synthetic_four_channel(path: &Path, sample_rate: u32, encoder: &[&str]) {
        let tones: Vec<String> = [220, 330, 440, 550]
            .iter()
            .zip(SYNTHETIC_CHANNEL_VOLUMES)
            .enumerate()
            .map(|(index, (frequency, volume))| {
                format!("{}[v{index}]", synthetic_tone(*frequency, sample_rate, volume))
            })
            .collect();
        let graph = format!(
            "{};[v0][v1][v2][v3]join=inputs=4:channel_layout=quad[out]",
            tones.join(";")
        );
        let mut args: Vec<String> = vec!["-filter_complex".into(), graph, "-map".into(), "[out]".into()];
        args.extend(encoder.iter().map(|arg| arg.to_string()));
        args.extend(["-y".into(), path.to_string_lossy().to_string()]);
        run_ffmpeg(&args);
    }

    /// A `SYNTHETIC_SECONDS` sine tone at `volume`, in 20 ms frames, as a
    /// filter-graph source (see `synthetic_four_channel` for why it is not a
    /// separate `-f lavfi` input).
    fn synthetic_tone(frequency: u32, sample_rate: u32, volume: f64) -> String {
        format!(
            "sine=frequency={frequency}:duration={SYNTHETIC_SECONDS}:sample_rate={sample_rate}:samples_per_frame={},volume={volume}",
            synthetic_frame(sample_rate)
        )
    }

    /// A synthetic 4-channel Ogg Opus stream in the SGMCA layout: mapping
    /// family 255, one uncoupled mono stream per channel, 16 kHz input tag.
    /// The tones are made at 16 kHz in 20 ms source frames, so libopus gets
    /// exactly 100 whole 20 ms frames with no resampler in between.
    fn synthetic_direct_audio(path: &Path) {
        synthetic_four_channel(
            path,
            16_000,
            &[
                "-c:a",
                "libopus",
                "-mapping_family",
                "255",
                "-frame_duration",
                "20",
                "-b:a",
                "128k",
                "-ar",
                "16000",
                "-f",
                "ogg",
            ],
        );
    }

    /// A synthetic SGMCA laid out as the interoperability notes describe it:
    /// the 40-byte file header and one segment (56-byte header, then the Ogg
    /// Opus stream). Returns the .sgmca path and the bare Ogg bytes.
    fn synthetic_sgmca(dir: &Path) -> (PathBuf, Vec<u8>) {
        let ogg = dir.join("synthetic_stream.ogg");
        synthetic_direct_audio(&ogg);
        let stream = fs::read(&ogg).unwrap();
        assert_eq!(&stream[..4], b"OggS");
        fs::remove_file(&ogg).unwrap();
        let sgmca = dir.join("synthetic_session.sgmca");
        fs::write(
            &sgmca,
            crate::sgmca::fixtures::direct_audio(&[("2026-11-22T10:15:30-0500", &stream)]),
        )
        .unwrap();
        (sgmca, stream)
    }

    /// A synthetic SGMCA in the older layout earlier DepoAudio builds wrote:
    /// a bare Ogg stream from byte 0, no headers.
    fn synthetic_plain_sgmca(dir: &Path) -> PathBuf {
        let sgmca = dir.join("synthetic_session.sgmca");
        synthetic_direct_audio(&sgmca);
        sgmca
    }

    /// FFmpeg input arguments for `path` (`... -i <input>`): an SGMCA goes
    /// through the app's own feed (its concat script and decoder options),
    /// anything else is read directly. The feed keeps the script alive.
    fn ffmpeg_input(path: &Path) -> (Vec<String>, AudioFeed) {
        let feed = prepare_audio_feed_with_cancel(path, FeedOptions::default(), None).unwrap();
        (feed.input_args(&[]), feed)
    }

    /// Interleaved 32-bit float PCM at 48 kHz, decoded by FFmpeg.
    fn decode_f32(path: &Path, channels: usize) -> Vec<Vec<f32>> {
        let (input, _guard) = ffmpeg_input(path);
        let output = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
            .args(&input)
            .args(["-f", "f32le", "-ar", "48000", "-"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let samples: Vec<f32> = output
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        (0..channels)
            .map(|channel| samples.iter().skip(channel).step_by(channels).copied().collect())
            .collect()
    }

    /// Interleaved 32-bit float PCM at 48 kHz from FFmpeg's default decoder.
    fn decode_f32_default(path: &Path, channels: usize) -> Vec<Vec<f32>> {
        let output = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-map", "0:a:0", "-f", "f32le", "-ar", "48000", "-"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let samples: Vec<f32> = output
            .stdout
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| f32::from_le_bytes(*bytes))
            .collect();
        (0..channels)
            .map(|channel| samples.iter().skip(channel).step_by(channels).copied().collect())
            .collect()
    }

    fn court_job(src: &Path, out_dir: &Path, mode: &str, format: &str) -> ConvertJob {
        let mut job = valid_job();
        job.src_path = src.to_string_lossy().to_string();
        job.out_dir = out_dir.to_string_lossy().to_string();
        job.mode = mode.into();
        job.format = format.into();
        job.labels = ["Bench", "Witness", "Plaintiff", "Defense"].map(String::from).to_vec();
        job.chan_vols = vec![1.0; 4];
        job
    }

    fn ffprobe_json(path: &Path, args: &[&str]) -> serde_json::Value {
        let feed = prepare_audio_feed_with_cancel(path, FeedOptions::default(), None).unwrap();
        let output = std::process::Command::new("ffprobe")
            .args(["-v", "quiet", "-print_format", "json"])
            .args(args)
            .args(crate::helpers::feed_format_args(feed.path()))
            .arg(feed.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "ffprobe failed on {}", path.display());
        serde_json::from_slice(&output.stdout).unwrap()
    }

    /// Peak level of a decoded file in dBFS, from FFmpeg's volumedetect.
    fn max_volume_db(path: &Path) -> f64 {
        max_volume_db_with(path, "volumedetect")
    }

    /// Peak level in dBFS between `start` and `end` seconds.
    fn max_volume_db_between(path: &Path, start: f64, end: f64) -> f64 {
        max_volume_db_with(path, &format!("atrim=start={start}:end={end},volumedetect"))
    }

    fn max_volume_db_with(path: &Path, filter: &str) -> f64 {
        let (input, _guard) = ffmpeg_input(path);
        let output = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner"])
            .args(&input)
            .args(["-af", filter, "-f", "null", "-"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let log = String::from_utf8_lossy(&output.stderr);
        let line = log
            .lines()
            .find(|line| line.contains("max_volume:"))
            .expect("volumedetect reports max_volume");
        line.split("max_volume:")
            .nth(1)
            .unwrap()
            .trim()
            .trim_end_matches("dB")
            .trim()
            .parse()
            .unwrap()
    }

    fn sha256_of(path: &Path) -> String {
        manifest::sha256_file(path, &|| false).unwrap().0
    }

    /// What one synthetic conversion produced.
    #[derive(Debug)]
    struct CourtConversion {
        files: Vec<OutputFile>,
        /// The feed's first input (removed by now if it was temporary).
        feed: PathBuf,
        /// Every input FFmpeg read.
        feed_inputs: Vec<String>,
        /// The FFmpeg arguments that ran.
        args: Vec<String>,
    }

    /// `do_convert` without the Tauri runtime: the same request checks, source
    /// preparation and fingerprint, probes (with the app's own ffprobe
    /// arguments), filters, plan and `finish_outputs`, with FFmpeg run from
    /// PATH instead of the bundled sidecar. `channel_gains` stands in for the
    /// auto-level analysis result, which the plan uses only when
    /// `job.auto_level` is on.
    fn convert_with_system_ffmpeg(
        job: &ConvertJob,
        channel_gains: Option<Vec<f64>>,
    ) -> Result<CourtConversion, String> {
        convert_with_system_ffmpeg_full(job, channel_gains, FeedOptions::default(), &|_| {})
    }

    /// As [`convert_with_system_ffmpeg`], with the feed options the app would
    /// pass (for an FTR recording: whether the native decoder is present).
    fn convert_with_system_ffmpeg_options(
        job: &ConvertJob,
        channel_gains: Option<Vec<f64>>,
        options: FeedOptions,
    ) -> Result<CourtConversion, String> {
        convert_with_system_ffmpeg_full(job, channel_gains, options, &|_| {})
    }

    /// `convert_with_system_ffmpeg`, with `edit` applied to the plan after
    /// FFmpeg ran and before the outputs are checked and recorded.
    fn convert_with_system_ffmpeg_edited(
        job: &ConvertJob,
        channel_gains: Option<Vec<f64>>,
        edit: &dyn Fn(&mut ConversionPlan),
    ) -> Result<CourtConversion, String> {
        convert_with_system_ffmpeg_full(job, channel_gains, FeedOptions::default(), edit)
    }

    fn convert_with_system_ffmpeg_full(
        job: &ConvertJob,
        channel_gains: Option<Vec<f64>>,
        options: FeedOptions,
        edit: &dyn Fn(&mut ConversionPlan),
    ) -> Result<CourtConversion, String> {
        validate_convert_request(job)?;
        let format = crate::helpers::detect_format_for_path(&job.src_path).ok_or("Unrecognised file format")?;
        assert!(
            format.handler != "rejected",
            "this harness covers readable formats only"
        );
        let (feed, source) = prepare_source(Path::new(&job.src_path), options, &|| false)?;
        let feed = finish_with_system_ffmpeg(feed)?;
        let sgmca_source = feed.sgmca().cloned();
        let (job, source) = with_dcr_facts(job, source, feed.dcr());
        let job = &job;

        let probe = |args: Vec<String>| {
            let output = std::process::Command::new("ffprobe").args(args).output().unwrap();
            assert!(output.status.success(), "ffprobe failed");
            output.stdout
        };
        // As do_convert_inner: the duration is always probed (progress and the
        // WAV size estimate); build_proc_filters_with_gain uses it only to
        // place the fade-out.
        // An FTR feed answers from the recording itself, as the app's probes do.
        let total_secs = if feed.trm_frames_read() {
            feed.known_duration()
        } else {
            crate::ffmpeg::parse_duration_probe(&probe(crate::ffmpeg::duration_probe_args(feed.path())))
        };
        let probed = manifest::probed_from_feed(&feed).unwrap_or_else(|| {
            let probed =
                serde_json::from_slice::<serde_json::Value>(&probe(crate::ffmpeg::manifest_probe_args(feed.path())))
                    .map(|value| manifest::parse_probe(&value))
                    .unwrap_or_default();
            manifest::with_trm_timing(probed, &feed)
        });
        // As build_proc_filters_with_gain: with Trim silence on, the app's own
        // measuring pass counts what trim removes, at the probed rate.
        let leading_trim = if crate::ffmpeg::trim_applies(job) {
            let measured = std::process::Command::new("ffmpeg")
                .args(crate::ffmpeg::leading_trim_measure_args(&feed, job.silence_thresh))
                .output()
                .unwrap();
            let counts = measured
                .status
                .success()
                .then(|| crate::ffmpeg::parse_leading_trim_counts(&measured.stderr))
                .flatten()
                .ok_or(crate::ffmpeg::TRIM_UNMEASURED_MESSAGE)?;
            let rate = probed.sample_rate.ok_or(crate::ffmpeg::TRIM_UNMEASURED_MESSAGE)?;
            Some(crate::ffmpeg::leading_trim_record(job, counts, rate)?)
        } else {
            None
        };
        let trimmed = leading_trim.as_ref().map_or(0.0, |trim| trim.seconds);
        // With Fade on, the decoded length (the app's own measuring pass)
        // places the fade-out.
        let fade_out = if job.fade {
            let measured = std::process::Command::new("ffmpeg")
                .args(crate::ffmpeg::decoded_length_args(&feed))
                .output()
                .unwrap();
            let decoded = measured
                .status
                .success()
                .then(|| crate::ffmpeg::parse_decoded_seconds(&measured.stderr))
                .flatten()
                .ok_or(crate::ffmpeg::FADE_LENGTH_UNKNOWN_MESSAGE)?;
            Some(crate::ffmpeg::fade_out_placement(job, decoded, trimmed, total_secs))
        } else {
            None
        };
        let built = crate::ffmpeg::proc_plan(job, None, fade_out, leading_trim);
        // As do_convert_inner: count the source's samples when an SGMCA
        // output needs them and no header states them.
        let source_samples = if needs_source_sample_count(
            job,
            &feed,
            sgmca_source.is_some(),
            trim_moves_start(built.leading_trim.as_ref()),
        ) {
            let counted = std::process::Command::new("ffmpeg")
                .args(crate::ffmpeg::source_length_args(&feed))
                .output()
                .unwrap();
            let samples = counted
                .status
                .success()
                .then(|| crate::ffmpeg::parse_decoded_samples(&counted.stderr))
                .flatten();
            Some(samples.ok_or(SOURCE_LENGTH_UNKNOWN_MESSAGE)?)
        } else {
            None
        };
        let probes = ConversionProbes {
            channels: feed
                .known_channels()
                .or_else(|| crate::ffmpeg::parse_channel_probe(&probe(crate::ffmpeg::channel_probe_args(feed.path())))),
            channel_gains,
            proc: built.filters,
            fade_out: built.fade_out,
            leading_trim: built.leading_trim,
            probed,
            duration_secs: total_secs,
            sgmca_source,
            source_samples,
        };

        let mut output_cleanup = OutputCleanup::default();
        let mut plan = plan_conversion(job, &feed, &probes, &mut output_cleanup)?;
        let mut args = plan.args.clone();
        args.splice(0..0, ["-loglevel".to_string(), "error".to_string()]);
        let status = std::process::Command::new("ffmpeg").args(&args).status().unwrap();
        if !status.success() {
            let error = format!("ffmpeg exited with {status}");
            return Err(plain_trm_decode_error(&feed, &error).unwrap_or(error));
        }
        edit(&mut plan);

        let input = ManifestInput {
            app_version: env!("CARGO_PKG_VERSION").into(),
            created_at: "2026-11-13T09:00:00Z".into(),
            source,
            probed: &probes.probed,
            labels: &job.labels,
        };
        let cancel = ConversionCancel::new(Arc::new(AtomicU64::new(1)), 1);
        let files = finish_outputs(job, &plan, &input, output_cleanup, &cancel)?;
        let first = feed.path().to_path_buf();
        let inputs = plan
            .args
            .windows(2)
            .filter(|pair| pair[0] == "-i")
            .map(|pair| pair[1].clone())
            .collect();
        drop(feed);
        Ok(CourtConversion {
            files,
            feed: first,
            feed_inputs: inputs,
            args: plan.args,
        })
    }

    /// Reads `<output>.depoaudio.json`, checks it against the published schema
    /// and the files on disk, and returns it.
    fn checked_export_record(output: &OutputFile, source: &Path) -> serde_json::Value {
        let path = manifest::manifest_path_for(Path::new(&output.path));
        let record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let errors = manifest::tests::schema_errors_for_value(&record);
        assert!(errors.is_empty(), "{}: {errors:#?}", path.display());
        assert_eq!(record["source"]["fileName"], basename(&source.to_string_lossy()));
        assert_eq!(record["source"]["sha256"], sha256_of(source));
        assert_eq!(record["source"]["sizeBytes"], fs::metadata(source).unwrap().len());
        assert_eq!(record["output"]["fileName"], output.name);
        assert_eq!(record["output"]["sha256"], sha256_of(Path::new(&output.path)));
        assert_eq!(record["output"]["sizeBytes"], output.size);
        let header = if record["output"]["format"] == "wav" {
            serde_json::json!(manifest::wav_header_kind(Path::new(&output.path)).unwrap())
        } else {
            serde_json::Value::Null
        };
        assert_eq!(record["output"]["wavHeader"], header);
        if record["output"]["format"] == "wav" {
            // A small WAV keeps the plain RIFF layout: `fmt ` right after
            // `WAVE`, no JUNK chunk reserved for RF64.
            let bytes = fs::read(&output.path).unwrap();
            assert_eq!(&bytes[8..16], b"WAVEfmt ", "{}", output.name);
            let encoder = record["processing"]["encoderArgs"].as_array().unwrap();
            assert!(!encoder.iter().any(|arg| arg == "-rf64"), "{}", output.name);
        }
        assert_eq!(record["channel"]["sourceChannelCount"], 4);
        // A synthetic source carries no bext start (nor, for FTR, a TLOC,
        // TUTC or FILE time), so none is claimed.
        if record["source"]["trm"]["startBasis"].is_null() {
            assert_eq!(record["startTime"]["known"], false);
        }
        record
    }

    fn assert_audio(path: &Path, channels: u64, sample_rate: &str) {
        let probe = ffprobe_json(path, &["-show_format", "-show_streams", "-select_streams", "a:0"]);
        assert_eq!(probe["streams"][0]["channels"], channels, "{}", path.display());
        assert_eq!(probe["streams"][0]["sample_rate"], sample_rate, "{}", path.display());
        let duration: f64 = probe["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!(
            (duration - SYNTHETIC_SECONDS).abs() < 0.1,
            "{} lasts {duration} s",
            path.display()
        );
    }

    /// Split outputs, in channel order, each one channel at the source's level
    /// for that channel (strictly quieter one by one), with a full record.
    fn assert_split_outputs(conversion: &CourtConversion, source: &Path, extension: &str) {
        let labels = ["Bench", "Witness", "Plaintiff", "Defense"];
        assert_eq!(conversion.files.len(), 4);
        let mut previous_peak = f64::INFINITY;
        for (index, (file, label)) in conversion.files.iter().zip(labels).enumerate() {
            assert_eq!(file.name, format!("synthetic_session_{label}{extension}"));
            let path = Path::new(&file.path);
            assert_audio(path, 1, "48000");
            let peak = max_volume_db(path);
            // lavfi's sine source peaks at 1/8 of full scale (about -18 dBFS).
            let expected = 20.0 * (SYNTHETIC_CHANNEL_VOLUMES[index] / 8.0).log10();
            assert!(
                (peak - expected).abs() < 1.5,
                "{label}: peak {peak} dB, expected {expected:.1} dB"
            );
            assert!(
                peak < previous_peak - 3.0,
                "{label} is not quieter than the channel before it"
            );
            previous_peak = peak;

            let record = checked_export_record(file, source);
            assert_eq!(record["processing"]["mode"], "split");
            assert_eq!(record["processing"]["filterOption"], "-filter_complex");
            assert_eq!(record["channel"]["output"], "single-channel");
            assert_eq!(record["channel"]["sourceChannelIndex"], index);
            assert_eq!(record["channel"]["label"], label);
        }
    }

    fn assert_mix_output(conversion: &CourtConversion, source: &Path, name: &str) {
        assert_eq!(conversion.files.len(), 1);
        let file = &conversion.files[0];
        assert_eq!(file.name, name);
        assert_audio(Path::new(&file.path), 2, "48000");
        let record = checked_export_record(file, source);
        assert_eq!(record["processing"]["mode"], "stereo");
        assert_eq!(record["processing"]["filterOption"], "-af");
        assert_eq!(record["channel"]["output"], "stereo-mix");
        assert_eq!(record["channel"]["sourceChannelIndex"], serde_json::Value::Null);
    }

    // ── Synthetic FTR recordings ────────────────────────────────────────────
    //
    // Built by `crate::trm::tests` from FFmpeg-encoded tones (300, 700, 1100,
    // 1700 Hz on channels 1-4), marked and scrambled as the interoperability
    // notes describe. No byte comes from a real FTR recording.

    use crate::trm::tests as trm_fixture;

    #[derive(Clone, Copy)]
    enum TrmLayout {
        /// One round per chunk, indexed, closed: what the native decoder takes.
        Aligned,
        /// Frames cut across chunks, and a channel-3 frame before channel 0.
        Straddling,
        /// No idx1, the audio length left at 0x7FFFFFFF, movi never closed.
        OpenEnded,
    }

    fn synthetic_trm(dir: &Path, layout: TrmLayout) -> PathBuf {
        let channels = trm_fixture::tone_channels(dir, SYNTHETIC_SECONDS);
        let (stream, lengths) = trm_fixture::ftr_frame_stream(&channels);
        let mut spec = trm_fixture::AviSpec::closed(4);
        let packets = match layout {
            TrmLayout::Aligned => trm_fixture::round_packets(&stream, &lengths, 4, 1),
            TrmLayout::Straddling => {
                let mut lead = channels[3][1].clone();
                lead[2] |= 0x02;
                lead.extend(&stream);
                trm_fixture::sized_packets(&lead, 900)
            }
            TrmLayout::OpenEnded => {
                spec.index = trm_fixture::Index::None;
                spec.length = 0x7FFF_FFFF;
                spec.open_movi = true;
                trm_fixture::round_packets(&stream, &lengths, 4, 1)
            }
        };
        let path = dir.join("synthetic_session.trm");
        fs::write(&path, trm_fixture::build_avi(&spec, &packets)).unwrap();
        path
    }

    fn native_options() -> FeedOptions {
        FeedOptions {
            native_ftr: true,
            native_failed: false,
        }
    }

    fn output_samples(path: &str) -> Vec<f32> {
        trm_fixture::decode_f32(&["-i".into(), path.into()])
    }

    fn assert_no_channel_files_left(inputs: &[String]) {
        for input in inputs {
            assert!(
                !Path::new(input).exists(),
                "temporary channel file left behind: {input}"
            );
        }
    }

    #[test]
    fn synthetic_trm_converts_the_same_by_native_ftr_and_by_split_channels() {
        if !trm_fixture::aac_tools_available() {
            return;
        }
        if !trm_fixture::native_ftr_decoder_on_path() {
            eprintln!("skipping: this FFmpeg has no native ftr decoder to cross-check against");
            return;
        }
        let dir = ConversionTestDir::new();
        let trm = synthetic_trm(&dir.0, TrmLayout::Aligned);
        let original = fs::read(&trm).unwrap();
        let native_out = dir.0.join("native");
        let split_out = dir.0.join("split");
        fs::create_dir(&native_out).unwrap();
        fs::create_dir(&split_out).unwrap();

        let native =
            convert_with_system_ffmpeg_options(&court_job(&trm, &native_out, "keep", "wav"), None, native_options())
                .unwrap();
        let split = convert_with_system_ffmpeg(&court_job(&trm, &split_out, "keep", "wav"), None).unwrap();
        assert_eq!(native.feed_inputs, vec![trm.to_string_lossy().to_string()]);
        assert!(native.args.windows(2).any(|pair| pair == ["-c:a", "ftr"]));
        assert_eq!(split.feed_inputs.len(), 4);
        assert!(split.args.iter().any(|arg| arg.contains("amerge=inputs=4")));
        assert_no_channel_files_left(&split.feed_inputs);

        let native_samples = output_samples(&native.files[0].path);
        let split_samples = output_samples(&split.files[0].path);
        assert_eq!(native_samples.len(), split_samples.len());
        assert!(
            native_samples
                .iter()
                .zip(&split_samples)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "Keep Original output differs between the native and split-channel routes"
        );
        assert_eq!(
            trm_fixture::dominant_tones(&split_samples, 4, 48_000),
            trm_fixture::TONE_FREQUENCIES.to_vec()
        );

        let native_record = checked_export_record(&native.files[0], &trm);
        assert_eq!(native_record["source"]["trm"]["decodePath"], "native-ftr");
        assert_eq!(native_record["source"]["trm"]["routeReasons"], serde_json::json!([]));
        assert_eq!(
            native_record["source"]["trm"]["inputOptions"],
            serde_json::json!(["-c:a", "ftr"])
        );
        assert_eq!(native_record["source"]["trm"]["inputCount"], 1);
        assert_eq!(
            native_record["processing"]["mapArgs"],
            serde_json::json!(["-map", "0:a:0"])
        );
        let split_record = checked_export_record(&split.files[0], &trm);
        assert_eq!(split_record["source"]["trm"]["decodePath"], "deinterleaved");
        assert_eq!(
            split_record["source"]["trm"]["routeReasons"],
            serde_json::json!(["native-decoder-unavailable"])
        );
        assert_eq!(split_record["processing"]["filterOption"], "-filter_complex");
        assert_eq!(
            split_record["processing"]["mapArgs"],
            serde_json::json!(["-map", "[da_out]"])
        );
        assert_eq!(split_record["source"]["trm"]["indexKind"], "idx1-absolute");
        assert_eq!(
            split_record["source"]["trm"]["inputOptions"],
            serde_json::json!(["-f", "aac"])
        );
        assert_eq!(split_record["source"]["trm"]["inputCount"], 4);
        assert_eq!(split_record["audioStream"]["sourceAudioStreamCount"], 1);
        assert_eq!(fs::read(&trm).unwrap(), original, "the original is never modified");
    }

    #[test]
    fn synthetic_trm_with_straddling_frames_splits_each_tone_to_its_own_file() {
        if !trm_fixture::aac_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let trm = synthetic_trm(&dir.0, TrmLayout::Straddling);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        // The native decoder is offered; the layout still routes around it.
        let conversion =
            convert_with_system_ffmpeg_options(&court_job(&trm, &out, "split", "wav"), None, native_options()).unwrap();
        assert_eq!(conversion.files.len(), 4);
        assert!(!conversion.args.iter().any(|arg| arg == "ftr"));
        assert_no_channel_files_left(&conversion.feed_inputs);
        for (index, file) in conversion.files.iter().enumerate() {
            let samples = output_samples(&file.path);
            assert_eq!(
                trm_fixture::dominant_tones(&samples, 1, 48_000),
                vec![trm_fixture::TONE_FREQUENCIES[index]],
                "{}",
                file.name
            );
            let record = checked_export_record(file, &trm);
            assert_eq!(record["channel"]["sourceChannelIndex"], index);
            let route = &record["source"]["trm"];
            assert_eq!(route["decodePath"], "deinterleaved");
            assert!(route["straddlingFrames"].as_u64().unwrap() > 0);
            assert!(route["resyncSkippedBytes"].as_u64().unwrap() > 0);
            let reasons = route["routeReasons"].as_array().unwrap();
            assert!(
                reasons.contains(&serde_json::json!("frames-cross-chunks")),
                "{reasons:?}"
            );
        }

        // Mix to Stereo hears every channel on both sides.
        let stereo_out = dir.0.join("stereo");
        fs::create_dir(&stereo_out).unwrap();
        let stereo = convert_with_system_ffmpeg(&court_job(&trm, &stereo_out, "stereo", "wav"), None).unwrap();
        let samples = output_samples(&stereo.files[0].path);
        for frequency in trm_fixture::TONE_FREQUENCIES {
            let on = trm_fixture::tone_power(&samples, 2, 0, frequency, 48_000);
            let off = trm_fixture::tone_power(&samples, 2, 0, frequency + 150, 48_000);
            assert!(on > off * 100.0, "{frequency} Hz is missing from the stereo mix");
        }
        let record = checked_export_record(&stereo.files[0], &trm);
        assert_eq!(record["processing"]["filterOption"], "-filter_complex");
        assert!(record["processing"]["filterChain"]
            .as_str()
            .unwrap()
            .starts_with("[0:a:0][1:a:0][2:a:0][3:a:0]amerge=inputs=4,aformat=sample_fmts=fltp,pan=stereo"));
    }

    /// The WAV output's bext TimeReference, as FFmpeg reads it back.
    fn wav_time_reference(path: &str) -> Option<u64> {
        let output = std::process::Command::new("ffprobe")
            .args(["-v", "quiet", "-print_format", "json", "-show_format", path])
            .output()
            .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        value["format"]["tags"]["time_reference"].as_str()?.parse().ok()
    }

    #[test]
    fn the_vendor_notes_sample_converts_natively_and_keeps_its_tloc_start() {
        if !trm_fixture::aac_tools_available() {
            return;
        }
        if !trm_fixture::native_ftr_decoder_on_path() {
            eprintln!("skipping: this FFmpeg has no native ftr decoder");
            return;
        }
        let dir = ConversionTestDir::new();
        let trm = trm_fixture::vendor_notes_sample();
        let original = fs::read(&trm).unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion =
            convert_with_system_ffmpeg_options(&court_job(&trm, &out, "keep", "wav"), None, native_options()).unwrap();
        assert!(conversion.args.windows(2).any(|pair| pair == ["-c:a", "ftr"]));
        let record = checked_export_record(&conversion.files[0], &trm);
        let route = &record["source"]["trm"];
        assert_eq!(route["decodePath"], "native-ftr");
        assert_eq!(route["startUtc"], "2026-01-01T09:00:00Z");
        assert_eq!(route["startBasis"], "tloc-local");
        assert_eq!(route["userNamePresent"], false);
        let start = &record["startTime"];
        assert_eq!(start["known"], true);
        assert_eq!(start["source"], "ftr-tloc");
        assert_eq!(start["secondsSinceMidnight"], 32_400.0);
        assert_eq!(start["writtenToOutput"], true);
        let rate = record["output"]["sampleRate"].as_u64().unwrap();
        assert_eq!(start["outputTimeReferenceSamples"], 9 * 3600 * rate);
        assert_eq!(wav_time_reference(&conversion.files[0].path), Some(9 * 3600 * rate));
        let samples = output_samples(&conversion.files[0].path);
        assert_eq!(samples.len() as u64, 15_360 * rate / 1000 * 4);
        assert_eq!(fs::read(&trm).unwrap(), original, "the original is never modified");
    }

    /// A copy of the owner's sample with `edit` applied to its bytes.
    fn vendor_sample_variant(dir: &Path, name: &str, edit: impl FnOnce(&mut Vec<u8>)) -> PathBuf {
        let mut bytes = fs::read(trm_fixture::vendor_notes_sample()).unwrap();
        edit(&mut bytes);
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn u32_le(bytes: &[u8], at: usize) -> usize {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
    }

    #[test]
    fn vendor_sample_variants_convert_in_full_and_keep_non_local_starts_out_of_bext() {
        if !trm_fixture::aac_tools_available() {
            return;
        }
        if !trm_fixture::native_ftr_decoder_on_path() {
            eprintln!("skipping: this FFmpeg has no native ftr decoder");
            return;
        }
        let dir = ConversionTestDir::new();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let full_samples = |path: &str, rate: u64| {
            assert_eq!(output_samples(path).len() as u64, 15_360 * rate / 1000 * 4, "{path}");
        };

        // idx1 cut to its first 100 of 240 entries: movi still holds all 240
        // chunks, which DepoAudio now finds by walking movi.
        let short = vendor_sample_variant(&dir.0, "short_index.trm", |bytes| {
            let idx1 = bytes.windows(4).rposition(|window| window == b"idx1").unwrap();
            bytes.truncate(idx1 + 8 + 100 * 16);
            bytes[idx1 + 4..idx1 + 8].copy_from_slice(&1600u32.to_le_bytes());
            let riff = (bytes.len() - 8) as u32;
            bytes[4..8].copy_from_slice(&riff.to_le_bytes());
        });
        let conversion =
            convert_with_system_ffmpeg_options(&court_job(&short, &out, "keep", "wav"), None, native_options())
                .unwrap();
        let record = checked_export_record(&conversion.files[0], &short);
        let route = &record["source"]["trm"];
        assert_eq!(route["indexMismatch"], true);
        assert_eq!(route["indexKind"], "walked");
        assert_eq!(route["packets"], 240);
        assert_eq!(route["durationSecs"], 15.36);
        assert_eq!(route["decodePath"], "deinterleaved");
        assert!(route["routeReasons"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("index-misses-audio-chunks")));
        let rate = record["output"]["sampleRate"].as_u64().unwrap();
        full_samples(&conversion.files[0].path, rate);

        // The private bit set on every frame: DepoAudio finds no stream-0
        // frame to split from, so FFmpeg's native decoder (which does not use
        // the bit) reads the whole file, and the record says why.
        let all_private = vendor_sample_variant(&dir.0, "all_private.trm", |bytes| {
            let mut pos = bytes.windows(4).position(|window| window == b"movi").unwrap() + 4;
            while &bytes[pos..pos + 4] == b"00wb" {
                let size = u32_le(bytes, pos + 4);
                let mut frame = pos + 8;
                while frame < pos + 8 + size {
                    bytes[frame + 2] |= 0x02;
                    frame += ((usize::from(bytes[frame + 4]) << 8) | usize::from(bytes[frame + 5])) >> 5;
                }
                pos += 8 + size + (size & 1);
            }
        });
        let conversion =
            convert_with_system_ffmpeg_options(&court_job(&all_private, &out, "keep", "wav"), None, native_options())
                .unwrap();
        assert!(conversion.args.windows(2).any(|pair| pair == ["-c:a", "ftr"]));
        let record = checked_export_record(&conversion.files[0], &all_private);
        assert!(record["source"].get("trm").is_none());
        assert_eq!(
            record["source"]["trmInspectionError"],
            crate::trm::NO_COMPLETE_ROUND_MESSAGE
        );
        let rate = record["output"]["sampleRate"].as_u64().unwrap();
        full_samples(&conversion.files[0].path, rate);

        // No TLOC chunk: the start is UTC, which a bext time reference (local
        // midnight) cannot carry, so the WAV gets none and the record keeps it.
        let utc_only = vendor_sample_variant(&dir.0, "utc_only.trm", |bytes| {
            let tloc = bytes.windows(4).position(|window| window == b"TLOC").unwrap();
            bytes[tloc..tloc + 4].copy_from_slice(b"JUNK");
        });
        let conversion =
            convert_with_system_ffmpeg_options(&court_job(&utc_only, &out, "keep", "wav"), None, native_options())
                .unwrap();
        let record = checked_export_record(&conversion.files[0], &utc_only);
        assert_eq!(record["source"]["trm"]["startBasis"], "tutc-utc");
        let start = &record["startTime"];
        assert_eq!(
            (start["known"].clone(), start["source"].clone()),
            (serde_json::json!(true), serde_json::json!("ftr-tutc"))
        );
        assert_eq!(start["writtenToOutput"], false);
        assert!(start["note"].as_str().unwrap().contains("not local time"));
        assert_eq!(wav_time_reference(&conversion.files[0].path), None);
        assert!(!conversion.args.iter().any(|arg| arg == "-write_bext"));
    }

    #[test]
    fn a_stereo_stream_trm_splits_to_one_file_per_microphone_in_order() {
        if !trm_fixture::aac_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let [a, b, c, d] = trm_fixture::TONE_FREQUENCIES;
        let streams = vec![
            trm_fixture::stereo_tone_frames(&dir.0, a, b, SYNTHETIC_SECONDS),
            trm_fixture::stereo_tone_frames(&dir.0, c, d, SYNTHETIC_SECONDS),
        ];
        let (stream, lengths) = trm_fixture::ftr_frame_stream(&streams);
        let mut spec = trm_fixture::AviSpec::closed(4);
        spec.tag = 0x8180;
        let trm = dir.0.join("stereo_streams.trm");
        fs::write(
            &trm,
            trm_fixture::build_avi(&spec, &trm_fixture::round_packets(&stream, &lengths, 2, 1)),
        )
        .unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        // Without the native decoder: two stereo stream files joined by amerge.
        let conversion = convert_with_system_ffmpeg(&court_job(&trm, &out, "split", "wav"), None).unwrap();
        assert_eq!(conversion.feed_inputs.len(), 2);
        assert!(conversion.args.iter().any(|arg| arg.contains("amerge=inputs=2")));
        assert_no_channel_files_left(&conversion.feed_inputs);
        assert_eq!(conversion.files.len(), 4);
        for (index, file) in conversion.files.iter().enumerate() {
            let samples = output_samples(&file.path);
            assert_eq!(
                trm_fixture::dominant_tones(&samples, 1, 48_000),
                vec![trm_fixture::TONE_FREQUENCIES[index]],
                "{}",
                file.name
            );
            let record = checked_export_record(file, &trm);
            let route = &record["source"]["trm"];
            assert_eq!(route["streamLayout"], "stereo-streams");
            assert_eq!(
                (route["streams"].as_u64(), route["channelsPerStream"].as_u64()),
                (Some(2), Some(2))
            );
            assert_eq!(route["inputCount"], 2);
            assert_eq!(record["channel"]["sourceChannelIndex"], index);
        }
    }

    #[test]
    fn mp2_audio_in_an_ftr_file_is_converted_as_ordinary_audio_and_bad_mp3_is_named() {
        if !trm_fixture::aac_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let mp2 = dir.0.join("tone.mp2");
        let status = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=1:sample_rate=16000")
            .args(["-c:a", "mp2", "-b:a", "64k", "-f", "mp2", "-y"])
            .arg(&mp2)
            .status()
            .unwrap();
        assert!(status.success());
        let mut spec = trm_fixture::AviSpec::closed(1);
        spec.tag = 0x0050;
        let trm = dir.0.join("mp2_session.trm");
        fs::write(&trm, trm_fixture::build_avi(&spec, &[fs::read(&mp2).unwrap()])).unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let mut job = court_job(&trm, &out, "keep", "wav");
        job.labels.truncate(1);
        job.chan_vols.truncate(1);
        let conversion = convert_with_system_ffmpeg_options(&job, None, native_options()).unwrap();
        assert!(!conversion.args.iter().any(|arg| arg == "ftr"));
        assert_eq!(conversion.feed_inputs, vec![trm.to_string_lossy().to_string()]);
        let samples = output_samples(&conversion.files[0].path);
        assert!(trm_fixture::tone_power(&samples, 1, 0, 440, 48_000) > 1.0);
        let record_path = manifest::manifest_path_for(Path::new(&conversion.files[0].path));
        let record: serde_json::Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
        let errors = manifest::tests::schema_errors_for_value(&record);
        assert!(errors.is_empty(), "{errors:#?}");
        let route = &record["source"]["trm"];
        assert_eq!(route["decodePath"], "ffmpeg-plain");
        assert_eq!(route["audioCodec"], "mp2");
        assert_eq!(route["routeReasons"], serde_json::json!(["not-ftr-aac"]));
        assert_eq!(route["inputOptions"], serde_json::json!([]));
        assert_eq!(record["channel"]["sourceChannelCount"], 1);

        // MP3-tagged bytes FFmpeg cannot decode: the message names the codec.
        spec.tag = 0x0055;
        let bad = dir.0.join("bad_mp3.trm");
        fs::write(&bad, trm_fixture::build_avi(&spec, &[vec![0x11; 400]])).unwrap();
        let bad_out = dir.0.join("bad");
        fs::create_dir(&bad_out).unwrap();
        let mut job = court_job(&bad, &bad_out, "keep", "wav");
        job.labels.truncate(1);
        job.chan_vols.truncate(1);
        let error = convert_with_system_ffmpeg_options(&job, None, native_options())
            .err()
            .unwrap();
        assert!(error.contains("MP3"), "{error}");
    }

    #[test]
    fn an_ftr_aac_tag_without_layout_bits_is_refused_even_with_the_native_decoder() {
        let dir = ConversionTestDir::new();
        let channels = trm_fixture::fake_frames(2, 3);
        let (stream, lengths) = trm_fixture::ftr_frame_stream(&channels);
        let mut spec = trm_fixture::AviSpec::closed(2);
        spec.tag = 0x0180;
        let trm = dir.0.join("no_layout_bits.trm");
        fs::write(
            &trm,
            trm_fixture::build_avi(&spec, &trm_fixture::round_packets(&stream, &lengths, 2, 1)),
        )
        .unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let error = convert_with_system_ffmpeg_options(&court_job(&trm, &out, "keep", "wav"), None, native_options())
            .err()
            .unwrap();
        assert_eq!(error, crate::trm::TRM_UNSPLITTABLE_MESSAGE);
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "nothing is written");
    }

    #[test]
    fn an_unclosed_unindexed_trm_converts_the_audio_that_was_written() {
        if !trm_fixture::aac_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let trm = synthetic_trm(&dir.0, TrmLayout::OpenEnded);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let mut job = court_job(&trm, &out, "keep", "flac");
        job.fade = true;
        let conversion = convert_with_system_ffmpeg_options(&job, None, native_options()).unwrap();
        let record = checked_export_record(&conversion.files[0], &trm);
        let route = &record["source"]["trm"];
        assert_eq!(route["openEnded"], true);
        assert_eq!(route["indexKind"], "walked");
        assert_eq!(route["decodePath"], "deinterleaved");
        let frames = route["framesPerChannel"].as_u64().unwrap();
        let expected = frames as f64 * 1024.0 / 16_000.0;
        assert_eq!(route["durationSecs"].as_f64().unwrap(), expected);
        // The fade-out sits at the end of the audio read, not at a duration
        // worked out from the 0x7FFFFFFF placeholder.
        let chain = record["processing"]["filterChain"].as_str().unwrap();
        assert!(
            chain.contains(&format!("afade=t=out:st={:.3}", expected - job.fade_dur)),
            "{chain}"
        );
        let samples = output_samples(&conversion.files[0].path);
        assert_eq!(samples.len() as f64 / 4.0 / 48_000.0, expected);
        assert_no_channel_files_left(&conversion.feed_inputs);
    }

    #[test]
    fn synthetic_sgmca_splits_into_one_file_per_channel_with_records() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (sgmca, stream) = synthetic_sgmca(&dir.0);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let original = fs::read(&sgmca).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&sgmca, &out, "split", "wav"), None).unwrap();
        assert_split_outputs(&conversion, &sgmca, ".wav");

        // The record fingerprints the .sgmca as selected, not its Ogg
        // stream; the original is untouched; the temporary script is gone.
        let record = checked_export_record(&conversion.files[0], &sgmca);
        let stream_digest = {
            let bare = dir.0.join("bare_stream.ogg");
            fs::write(&bare, &stream).unwrap();
            sha256_of(&bare)
        };
        assert_ne!(record["source"]["sha256"], stream_digest);
        assert_eq!(fs::read(&sgmca).unwrap(), original);
        assert_ne!(conversion.feed, sgmca);
        assert!(!conversion.feed.exists(), "the concat script was left behind");
        let layout = &record["source"]["sgmca"];
        assert_eq!(layout["fileVersion"], 1);
        assert_eq!(layout["guidPresent"], true);
        assert_eq!(layout["growing"], false);
        assert_eq!(layout["legacyBareOgg"], false);
        assert_eq!(layout["segments"].as_array().unwrap().len(), 1);
        assert_eq!(layout["segments"][0]["startLocal"], "2026-11-22T10:15:30");
        assert_eq!(layout["segments"][0]["utcOffset"], "-05:00");
        assert_eq!(layout["segments"][0]["mappingFamily"], 255);
        assert!(record["output"].get("sgmca").is_none());
    }

    #[test]
    fn synthetic_sgmca_mixes_to_stereo_with_a_record() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (sgmca, _) = synthetic_sgmca(&dir.0);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&sgmca, &out, "stereo", "wav"), None).unwrap();
        assert_mix_output(&conversion, &sgmca, "synthetic_session.wav");
        assert!(!conversion.feed.exists(), "the concat script was left behind");
    }

    #[test]
    fn synthetic_sgmca_split_to_flac_and_a_second_run_never_replaces_the_first() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (sgmca, _) = synthetic_sgmca(&dir.0);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let job = court_job(&sgmca, &out, "split", "flac");

        let first = convert_with_system_ffmpeg(&job, None).unwrap();
        assert_split_outputs(&first, &sgmca, ".flac");
        let first_digests: Vec<String> = first
            .files
            .iter()
            .map(|file| sha256_of(Path::new(&file.path)))
            .collect();
        let record_bytes = |files: &[OutputFile]| -> Vec<Vec<u8>> {
            files
                .iter()
                .map(|file| fs::read(manifest::manifest_path_for(Path::new(&file.path))).unwrap())
                .collect()
        };
        let first_records = record_bytes(&first.files);

        let second = convert_with_system_ffmpeg(&job, None).unwrap();
        assert_eq!(second.files.len(), 4);
        for (before, after) in first.files.iter().zip(&second.files) {
            assert_ne!(before.path, after.path, "a second conversion reused an output name");
            assert_ne!(
                manifest::manifest_path_for(Path::new(&before.path)),
                manifest::manifest_path_for(Path::new(&after.path)),
                "a second conversion reused an export record name"
            );
            checked_export_record(after, &sgmca);
        }
        let still: Vec<String> = first
            .files
            .iter()
            .map(|file| sha256_of(Path::new(&file.path)))
            .collect();
        assert_eq!(still, first_digests, "a second conversion changed the first outputs");
        assert_eq!(
            record_bytes(&first.files),
            first_records,
            "a second conversion changed the first export records"
        );
        // Each first record still describes its own output.
        for file in &first.files {
            checked_export_record(file, &sgmca);
        }
    }

    #[test]
    fn synthetic_sgmca_without_an_ogg_stream_fails_before_any_output() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let sgmca = dir.0.join("synthetic_broken.sgmca");
        fs::write(&sgmca, synthetic_vendor_prefix()).unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let error = convert_with_system_ffmpeg(&court_job(&sgmca, &out, "split", "wav"), None)
            .expect_err("a file with no Direct Audio header must not convert");
        assert!(error.contains("not a Direct Audio file"), "{error}");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    }

    /// A synthetic legal-video container: generated test-pattern video and a
    /// stereo sine tone, muxed with the container's usual codecs.
    fn synthetic_video_container(path: &Path, video_codec: &str, audio_codec: &str, muxer: &[&str]) {
        let mut args: Vec<String> = [
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=64x48:rate=25:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2:sample_rate=48000",
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-ac",
            "2",
            "-c:v",
            video_codec,
            "-c:a",
            audio_codec,
        ]
        .map(String::from)
        .to_vec();
        args.extend(muxer.iter().map(|arg| arg.to_string()));
        args.extend(["-y".into(), path.to_string_lossy().to_string()]);
        run_ffmpeg(&args);
    }

    #[test]
    fn synthetic_legacy_video_containers_convert_to_audio_only_outputs() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let cases: [(&str, &str, &str, &[&str]); 4] = [
            ("deposition.mpg", "mpeg1video", "mp2", &["-f", "mpeg"]),
            ("VTS_01_1.vob", "mpeg2video", "ac3", &["-f", "vob"]),
            (
                "00001.m2ts",
                "mpeg2video",
                "ac3",
                &["-f", "mpegts", "-mpegts_m2ts_mode", "1"],
            ),
            ("deposition.wmv", "wmv2", "wmav2", &["-f", "asf"]),
        ];
        for (name, video_codec, audio_codec, muxer) in cases {
            let source = dir.0.join(name);
            synthetic_video_container(&source, video_codec, audio_codec, muxer);
            let before = sha256_of(&source);
            for (mode, format) in [("keep", "m4a"), ("stereo", "wav")] {
                let out = dir.0.join(format!("{name}-{mode}"));
                fs::create_dir(&out).unwrap();
                let mut job = court_job(&source, &out, mode, format);
                job.labels.truncate(2);
                job.chan_vols.truncate(2);
                let conversion =
                    convert_with_system_ffmpeg(&job, None).unwrap_or_else(|error| panic!("{name} {mode}: {error}"));
                assert_eq!(
                    conversion.feed, source,
                    "{name}: the container is passed to FFmpeg unchanged"
                );
                assert_eq!(conversion.files.len(), 1, "{name} {mode}");
                let output = &conversion.files[0];
                let streams = ffprobe_json(Path::new(&output.path), &["-show_streams"]);
                let streams = streams["streams"].as_array().unwrap();
                assert_eq!(streams.len(), 1, "{name} {mode}: {streams:?}");
                assert_eq!(streams[0]["codec_type"], "audio", "{name} {mode}");
                assert_eq!(streams[0]["channels"], 2, "{name} {mode}");

                let record_path = manifest::manifest_path_for(Path::new(&output.path));
                let record: serde_json::Value = serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
                let errors = manifest::tests::schema_errors_for_value(&record);
                assert!(errors.is_empty(), "{name} {mode}: {errors:#?}");
                assert_eq!(record["source"]["fileName"], name);
                let encoder = record["processing"]["encoderArgs"].as_array().unwrap();
                assert_eq!(encoder[..2], ["-vn", "-sn"], "{name} {mode}");
            }
            assert_eq!(sha256_of(&source), before, "{name}: the original is never modified");
        }
    }

    // ── Synthetic DigitalCAT .dm (FFmpeg on PATH) ──────────────────────────
    //
    // Every .dm here is built by the test from the layout in dm.rs (itself
    // from owner-supplied notes, not yet checked against a recorded file):
    // generated PCM, or GSM that FFmpeg's libgsm encoder made from a lavfi
    // tone. No byte of a real DigitalCAT file is used.

    /// FFmpeg and ffprobe are on PATH (no particular encoder needed).
    fn dm_fixture_tools_available() -> bool {
        let tools = std::process::Command::new("ffmpeg").arg("-version").output().is_ok()
            && std::process::Command::new("ffprobe").arg("-version").output().is_ok();
        if !tools {
            assert!(
                std::env::var_os("DEPOAUDIO_REQUIRE_FFMPEG_TESTS").is_none_or(|value| value != "1"),
                "DEPOAUDIO_REQUIRE_FFMPEG_TESTS=1, but ffmpeg/ffprobe is not on PATH"
            );
            eprintln!("skipping: ffmpeg/ffprobe not on PATH");
        }
        tools
    }

    /// FFmpeg can encode Microsoft GSM (libgsm_ms), to make GSM fixtures.
    fn gsm_encoder_available() -> bool {
        let available = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-encoders"])
            .output()
            .is_ok_and(|output| String::from_utf8_lossy(&output.stdout).contains("libgsm_ms"));
        if !available {
            assert!(
                std::env::var_os("DEPOAUDIO_REQUIRE_FFMPEG_TESTS").is_none_or(|value| value != "1"),
                "DEPOAUDIO_REQUIRE_FFMPEG_TESTS=1, but ffmpeg has no libgsm_ms encoder"
            );
            eprintln!("skipping: ffmpeg has no libgsm_ms encoder");
        }
        available
    }

    /// The `fmt ` and `data` chunk bodies of a RIFF WAVE file.
    fn wav_chunks(bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let (mut fmt, mut data) = (None, None);
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let id = &bytes[at..at + 4];
            let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
            match id {
                b"fmt " => fmt = Some(body.to_vec()),
                b"data" => data = Some(body.to_vec()),
                _ => {}
            }
            at += 8 + size + size % 2;
        }
        (fmt.expect("fmt chunk"), data.expect("data chunk"))
    }

    fn dm_job(src: &Path, out_dir: &Path, mode: &str, format: &str, rate: &str) -> ConvertJob {
        let mut job = valid_job();
        job.src_path = src.to_string_lossy().to_string();
        job.out_dir = out_dir.to_string_lossy().to_string();
        job.mode = mode.into();
        job.format = format.into();
        job.rate = rate.into();
        job
    }

    /// The export record of a .dm conversion, checked against the schema and
    /// the files on disk.
    fn checked_dm_record(output: &OutputFile, source: &Path) -> serde_json::Value {
        let path = manifest::manifest_path_for(Path::new(&output.path));
        let record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let errors = manifest::tests::schema_errors_for_value(&record);
        assert!(errors.is_empty(), "{}: {errors:#?}", path.display());
        assert_eq!(record["source"]["fileName"], basename(&source.to_string_lossy()));
        assert_eq!(record["source"]["sha256"], sha256_of(source));
        assert_eq!(record["source"]["sizeBytes"], fs::metadata(source).unwrap().len());
        assert_eq!(record["output"]["sha256"], sha256_of(Path::new(&output.path)));
        assert_eq!(record["startTime"]["known"], false);
        record
    }

    const DM_T0: u32 = 1_763_800_000; // 2025-11-22T08:26:40Z

    #[test]
    fn synthetic_pcm_dm_converts_to_its_samples_minus_recorded_over_blocks() {
        if !dm_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        // 16-bit mono at 11025 Hz: the block rule gives 2756 frames, 5512 bytes.
        let format = crate::dm::tests::pcm_format(11025);
        let frames = 2756usize;
        let blocks: Vec<Vec<u8>> = (0..10)
            .map(|block| {
                (0..frames)
                    .flat_map(|frame| {
                        let t = (block * frames + frame) as f64 / 11025.0;
                        let level = 0.1 + 0.05 * block as f64;
                        let sample = (level * (2.0 * std::f64::consts::PI * 440.0 * t).sin() * 32767.0) as i16;
                        sample.to_le_bytes()
                    })
                    .collect()
            })
            .collect();
        let stamps = [
            DM_T0,
            DM_T0,
            DM_T0 + 1,
            0, // recorded over
            0, // recorded over
            DM_T0 + 2,
            DM_T0 + 30, // pause
            DM_T0 + 30,
            // The record-over that erased blocks 3 and 4: its new blocks are
            // appended at the end with the later clock (the notes' rule).
            DM_T0 + 120,
            DM_T0 + 120,
        ];
        let layout: Vec<(u32, &[u8])> = stamps.iter().zip(&blocks).map(|(s, b)| (*s, b.as_slice())).collect();
        let dm = dir.0.join("synthetic_session.dm");
        fs::write(&dm, crate::dm::tests::dm_bytes(&format, 5512, &layout)).unwrap();
        let before = sha256_of(&dm);
        let expected: Vec<u8> = stamps
            .iter()
            .zip(&blocks)
            .filter(|(stamp, _)| **stamp != 0)
            .flat_map(|(_, block)| block.iter().copied())
            .collect();

        let format_info = crate::helpers::detect_format_for_path(&dm.to_string_lossy()).unwrap();
        assert_eq!(format_info.key, "digitalcat");
        assert_eq!(format_info.status, "experimental");
        let streams = crate::dm::streams_info(&dm).unwrap();
        assert_eq!(streams.streams[0].sample_rate, Some(11025));
        assert_eq!(streams.dm.as_ref().unwrap().blocks_erased, 2);
        assert!(
            streams.dm.as_ref().unwrap().stamps_monotonic,
            "a record-over alone does not make the kept stamps go backwards"
        );
        assert!((streams.duration_secs.unwrap() - 8.0 * 2756.0 / 11025.0).abs() < 1e-9);

        let keep_out = dir.0.join("keep");
        fs::create_dir(&keep_out).unwrap();
        let keep = convert_with_system_ffmpeg(&dm_job(&dm, &keep_out, "keep", "wav", "11025"), None).unwrap();
        assert_ne!(keep.feed, dm);
        assert!(!keep.feed.exists(), "the temporary WAV is removed after the conversion");
        assert_eq!(keep.files.len(), 1);
        assert_eq!(keep.files[0].name, "synthetic_session_orig.wav");
        let (_, data) = wav_chunks(&fs::read(&keep.files[0].path).unwrap());
        assert!(
            data == expected,
            "the output is the input PCM without the erased blocks, in file order"
        );

        let record = checked_dm_record(&keep.files[0], &dm);
        assert_eq!(record["channel"]["sourceChannelCount"], 1);
        let timeline = &record["source"]["dmTimeline"];
        assert_eq!(timeline["formatTag"], 1);
        assert_eq!(timeline["codecName"], "pcm_s16le");
        assert_eq!(timeline["blockSize"], 5512);
        assert_eq!(timeline["blocksTotal"], 10);
        assert_eq!(timeline["blocksErased"], 2);
        assert_eq!(timeline["shortFinalBlock"], false);
        assert_eq!(timeline["stampsMonotonic"], true);
        assert_eq!(
            timeline["segments"],
            serde_json::json!([
                { "startSample": 0, "firstStampUtc": "2025-11-22T08:26:40Z", "blocks": 4 },
                { "startSample": 4 * 2756, "firstStampUtc": "2025-11-22T08:27:10Z", "blocks": 2 },
                { "startSample": 6 * 2756, "firstStampUtc": "2025-11-22T08:28:40Z", "blocks": 2 }
            ])
        );

        // Mix to Stereo as MP3 reads the same feed.
        let mix_out = dir.0.join("mix");
        fs::create_dir(&mix_out).unwrap();
        let mix = convert_with_system_ffmpeg(&dm_job(&dm, &mix_out, "stereo", "mp3", "44100"), None).unwrap();
        assert_eq!(mix.files[0].name, "synthetic_session.mp3");
        let probe = ffprobe_json(Path::new(&mix.files[0].path), &["-show_format", "-show_streams"]);
        assert_eq!(probe["streams"][0]["channels"], 2);
        let duration: f64 = probe["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!((duration - 8.0 * 0.25).abs() < 0.1, "mp3 lasts {duration} s");
        checked_dm_record(&mix.files[0], &dm);

        assert_eq!(sha256_of(&dm), before, "the original is never modified");
    }

    #[test]
    fn a_dm_that_is_not_one_is_refused_before_any_output() {
        if !dm_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let dm = dir.0.join("not_audio.dm");
        fs::write(&dm, b"DigitalCAT job notes, not audio").unwrap();
        let format = crate::helpers::detect_format_for_path(&dm.to_string_lossy()).unwrap();
        assert_eq!(format.key, "digitalcat-unrecognized");
        assert_eq!(format.handler, "rejected");
        // Preparation (what the conversion, scan and waveform all call) gives
        // the plain message too.
        let error = crate::helpers::prepare_audio_feed_with_cancel(&dm, FeedOptions::default(), None).unwrap_err();
        assert!(error.starts_with(crate::dm::DM_HEADER_UNRECOGNIZED), "{error}");

        // Every block recorded over: nothing to convert, nothing written.
        let erased = dir.0.join("all_erased.dm");
        let block = vec![0u8; 5512];
        let format = crate::dm::tests::pcm_format(11025);
        fs::write(
            &erased,
            crate::dm::tests::dm_bytes(&format, 5512, &[(0, &block), (0, &block)]),
        )
        .unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let error = convert_with_system_ffmpeg(&dm_job(&erased, &out, "keep", "wav", "11025"), None)
            .err()
            .unwrap();
        assert!(error.contains("no audio left"), "{error}");
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    }

    /// A tone encoded by FFmpeg's libgsm_ms at 8000 Hz (FFmpeg's GSM encoder
    /// only takes 8 kHz): the WAVEFORMATEX and the GSM data.
    fn synthetic_gsm(dir: &Path) -> (Vec<u8>, Vec<u8>, PathBuf) {
        let wav = dir.join("synthetic_gsm.wav");
        run_ffmpeg(&[
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            "sine=frequency=440:duration=2:sample_rate=8000".into(),
            "-ac".into(),
            "1".into(),
            "-c:a".into(),
            "libgsm_ms".into(),
            "-f".into(),
            "wav".into(),
            "-y".into(),
            wav.to_string_lossy().to_string(),
        ]);
        let (fmt, data) = wav_chunks(&fs::read(&wav).unwrap());
        assert_eq!(
            u16::from_le_bytes([fmt[0], fmt[1]]),
            0x0031,
            "FFmpeg wrote Microsoft GSM"
        );
        assert_eq!(u16::from_le_bytes([fmt[12], fmt[13]]), 65);
        assert_eq!(data.len() % 65, 0);
        (fmt, data, wav)
    }

    /// The decoded samples (16-bit little-endian) of `path` at its own rate.
    fn decoded_pcm(path: &Path) -> Vec<u8> {
        let output = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-loglevel", "error", "-i"])
            .arg(path)
            .args(["-map", "0:a:0", "-f", "s16le", "-acodec", "pcm_s16le", "-"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        output.stdout
    }

    /// GSM data cut into `block_size` blocks with rising stamps (4 per second).
    fn gsm_blocks(data: &[u8], block_size: usize) -> Vec<(u32, &[u8])> {
        data.chunks(block_size)
            .enumerate()
            .map(|(index, chunk)| (DM_T0 + index as u32 / 4, chunk))
            .collect()
    }

    #[test]
    fn synthetic_gsm_dm_decodes_exactly_like_the_gsm_wav_it_was_made_from() {
        if !dm_fixture_tools_available() || !gsm_encoder_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (fmt, data, gsm_wav) = synthetic_gsm(&dir.0);
        // 8000 Hz GSM: nAvgBytesPerSec 1625, so (1625 / 65) >> 2 = 6 codec
        // blocks, 390 bytes, per .dm block.
        assert_eq!(u32::from_le_bytes(fmt[8..12].try_into().unwrap()), 1625);
        let mut format = fmt.clone();
        if format.len() < 20 {
            format = crate::dm::tests::wave_format_bytes(0x31, 1, 8000, 1625, 65, 0, &320u16.to_le_bytes());
        }
        let blocks = gsm_blocks(&data, 390);
        let dm = dir.0.join("synthetic_gsm.dm");
        fs::write(&dm, crate::dm::tests::dm_bytes(&format, 390, &blocks)).unwrap();
        let parsed = crate::dm::parse_dm(&dm, &|| false).unwrap();
        // 50 codec blocks in 6-block .dm blocks leave a short last block,
        // which is kept with a warning.
        assert!(
            parsed
                .warnings
                .iter()
                .all(|warning| warning.starts_with("The last block is shorter")),
            "{:?}",
            parsed.warnings
        );

        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&dm_job(&dm, &out, "keep", "wav", "8000"), None).unwrap();
        let (_, converted) = wav_chunks(&fs::read(&conversion.files[0].path).unwrap());
        let direct = decoded_pcm(&gsm_wav);
        assert!(!direct.is_empty());
        assert!(
            converted == direct,
            "the .dm decodes to the same samples as its GSM WAV"
        );

        let record = checked_dm_record(&conversion.files[0], &dm);
        let timeline = &record["source"]["dmTimeline"];
        assert_eq!(timeline["codecName"], "gsm_ms");
        assert_eq!(timeline["blockSize"], 390);
        assert_eq!(timeline["blocksErased"], 0);
        assert_eq!(timeline["stampsMonotonic"], true);
        assert_eq!(timeline["shortFinalBlock"], data.len() % 390 != 0);
        assert_eq!(timeline["segments"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_gsm_dm_with_an_11025_hz_header_decodes_at_11025_hz() {
        if !dm_fixture_tools_available() || !gsm_encoder_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (_, data, gsm_wav) = synthetic_gsm(&dir.0);
        // The SoundSync default header, around GSM data FFmpeg encoded at
        // 8 kHz: GSM frames decode the same whatever rate the header states.
        let format = crate::dm::tests::gsm_format();
        let blocks = gsm_blocks(&data, 520);
        let dm = dir.0.join("synthetic_default.dm");
        fs::write(&dm, crate::dm::tests::dm_bytes(&format, 520, &blocks)).unwrap();

        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&dm_job(&dm, &out, "keep", "wav", "11025"), None).unwrap();
        let output = Path::new(&conversion.files[0].path);
        let probe = ffprobe_json(output, &["-show_streams"]);
        assert_eq!(probe["streams"][0]["sample_rate"], "11025");
        let (_, converted) = wav_chunks(&fs::read(output).unwrap());
        assert_eq!(converted.len(), data.len() / 65 * 320 * 2);
        assert!(
            converted == decoded_pcm(&gsm_wav),
            "same samples, played at the stated rate"
        );
        let record = checked_dm_record(&conversion.files[0], &dm);
        assert_eq!(record["output"]["sampleRate"], 11025);
        assert_eq!(record["source"]["dmTimeline"]["blockSize"], 520);
    }

    #[test]
    fn the_owner_gsm_sample_decodes_to_the_owner_decoded_wav() {
        // A synthetic tone from the owner's rebuild tool, not a vendor or
        // court file (scripts/fixtures/dm/vendor-notes-samples/README.md).
        if !dm_fixture_tools_available() {
            return;
        }
        let samples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/fixtures/dm/vendor-notes-samples");
        let dm = samples.join("tone_gsm.dm");
        let before = sha256_of(&dm);
        let bmf_before = sha256_of(&samples.join("tone_gsm.bmf"));
        let parsed = crate::dm::parse_dm(&dm, &|| false).unwrap();
        assert_eq!(
            parsed.stamps,
            [vec![1_700_000_000u32; 5], vec![1_700_000_001; 4]].concat()
        );
        assert!(parsed.warnings.is_empty(), "{:?}", parsed.warnings);

        let dir = ConversionTestDir::new();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&dm_job(&dm, &out, "keep", "wav", "11025"), None).unwrap();
        let (fmt, converted) = wav_chunks(&fs::read(&conversion.files[0].path).unwrap());
        let (reference_fmt, reference) = wav_chunks(&fs::read(samples.join("tone_gsm_decoded.wav")).unwrap());
        assert_eq!(fmt[..16], reference_fmt[..16], "16-bit mono 11025 Hz PCM");
        assert_eq!(reference.len(), 23040 * 2);
        assert_eq!(converted.len(), reference.len());
        let first_difference = converted.iter().zip(&reference).position(|(a, b)| a != b);
        assert_eq!(first_difference, None, "sample for sample equal to the owner's decode");

        let record = checked_dm_record(&conversion.files[0], &dm);
        let timeline = &record["source"]["dmTimeline"];
        assert_eq!(timeline["blocksTotal"], 9);
        assert_eq!(timeline["blocksErased"], 0);
        assert_eq!(timeline["shortFinalBlock"], false);
        assert_eq!(
            timeline["segments"],
            serde_json::json!([{ "startSample": 0, "firstStampUtc": "2023-11-14T22:13:20Z", "blocks": 9 }])
        );
        let markers = &timeline["markerFile"];
        assert_eq!(markers["status"], "read");
        assert_eq!(markers["fileName"], "tone_gsm.bmf");
        assert_eq!(markers["entries"], 2);
        assert_eq!(markers["entriesMatchingABlock"], 2);
        let record_text =
            fs::read_to_string(manifest::manifest_path_for(Path::new(&conversion.files[0].path))).unwrap();
        assert!(
            !record_text.contains("Objection"),
            "marker text never reaches the record"
        );
        assert_eq!(sha256_of(&dm), before, "the original is never modified");
        assert_eq!(
            sha256_of(&samples.join("tone_gsm.bmf")),
            bmf_before,
            "nor is the marker file"
        );
    }

    /// A synthetic `.mkv` with two audio streams that differ in channel
    /// count: stream 0 is a mono 440 Hz tone at 0.5 (tagged eng, "Floor"),
    /// stream 1 a much quieter 4-channel stream (tagged spa, "Interpreter").
    /// Neither stream is marked default, so FFmpeg's automatic selection
    /// prefers the one with more channels: without an explicit map a
    /// conversion would encode stream 1. (FFmpeg 6.1 checked; a stream marked
    /// default wins over channel count, which is the same risk when the
    /// default track is not the first.)
    fn synthetic_two_stream_mkv(dir: &Path) -> PathBuf {
        let path = dir.join("synthetic_two_tracks.mkv");
        // The tones are graph sources so `join` keeps every frame (see
        // `synthetic_four_channel`).
        let graph = format!(
            "{}[floor];{}[q0];{}[q1];{}[q2];{}[q3];[q0][q1][q2][q3]join=inputs=4:channel_layout=quad[interp]",
            synthetic_tone(440, 48_000, 0.5),
            synthetic_tone(220, 48_000, 0.0625),
            synthetic_tone(330, 48_000, 0.0625),
            synthetic_tone(550, 48_000, 0.0625),
            synthetic_tone(660, 48_000, 0.0625),
        );
        let mut args: Vec<String> = vec!["-filter_complex".into(), graph];
        for (label, extra) in [
            (
                "[floor]",
                ["-metadata:s:a:0", "language=eng", "-metadata:s:a:0", "title=Floor"],
            ),
            (
                "[interp]",
                [
                    "-metadata:s:a:1",
                    "language=spa",
                    "-metadata:s:a:1",
                    "title=Interpreter",
                ],
            ),
        ] {
            args.extend(["-map".into(), label.into()]);
            args.extend(extra.iter().map(|arg| arg.to_string()));
        }
        args.extend(
            [
                "-c:a",
                "pcm_s16le",
                "-disposition:a:0",
                "0",
                "-disposition:a:1",
                "0",
                "-y",
            ]
            .map(String::from),
        );
        args.push(path.to_string_lossy().to_string());
        run_ffmpeg(&args);
        path
    }

    fn assert_records_stream_zero_of_two(file: &OutputFile, source: &Path) {
        let path = manifest::manifest_path_for(Path::new(&file.path));
        let record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let errors = manifest::tests::schema_errors_for_value(&record);
        assert!(errors.is_empty(), "{}: {errors:#?}", path.display());
        assert_eq!(record["source"]["sha256"], sha256_of(source));
        assert_eq!(record["output"]["sha256"], sha256_of(Path::new(&file.path)));
        assert_eq!(record["audioStream"]["index"], 0);
        assert_eq!(record["audioStream"]["sourceAudioStreamCount"], 2);
        assert_eq!(record["channel"]["sourceChannelCount"], 1);
        assert_eq!(record["output"]["wavHeader"], "riff");
    }

    #[test]
    fn plans_map_the_converted_audio_stream_in_every_mode() {
        let dir = ConversionTestDir::new();
        let feed = dir.0.join("tracks.mkv");
        let probes = ConversionProbes {
            channels: Some(2),
            channel_gains: None,
            proc: Vec::new(),
            fade_out: None,
            leading_trim: None,
            probed: manifest::ProbedSource::default(),
            duration_secs: None,
            sgmca_source: None,
            source_samples: None,
        };
        let feed_arg = feed.to_string_lossy().to_string();
        for mode in ["stereo", "keep"] {
            let mut job = valid_job();
            job.src_path = feed_arg.clone();
            job.out_dir = dir.0.to_string_lossy().to_string();
            job.mode = mode.into();
            let mut cleanup = OutputCleanup::default();
            let plan = plan_conversion(&job, &AudioFeed::plain(feed.to_path_buf()), &probes, &mut cleanup).unwrap();
            let input = plan.args.iter().position(|arg| arg == &feed_arg).unwrap();
            assert_eq!(plan.args[input - 1], "-i");
            assert_eq!(
                plan.args[input + 1..input + 3],
                ["-map", "0:a:0"],
                "{mode}: {:?}",
                plan.args
            );
            assert_eq!(plan.args.iter().filter(|arg| *arg == "-map").count(), 1, "{mode}");
        }
        let mut job = valid_job();
        job.src_path = feed_arg.clone();
        job.out_dir = dir.0.to_string_lossy().to_string();
        job.mode = "split".into();
        let mut cleanup = OutputCleanup::default();
        let plan = plan_conversion(&job, &AudioFeed::plain(feed.to_path_buf()), &probes, &mut cleanup).unwrap();
        assert!(plan.filter_chain.as_deref().unwrap().starts_with("[0:a:0]asplit=2"));
        let maps: Vec<&String> = plan
            .args
            .windows(2)
            .filter(|pair| pair[0] == "-map")
            .map(|pair| &pair[1])
            .collect();
        assert_eq!(maps, ["[op0]", "[op1]"]);
        assert_eq!(plan.map_args, vec![vec!["-map", "[op0]"], vec!["-map", "[op1]"]]);
    }

    #[test]
    fn plans_record_each_outputs_map_and_use_rf64_only_for_large_wavs() {
        let dir = ConversionTestDir::new();
        let feed = dir.0.join("session.wav");
        let plan_for = |mode: &str, format: &str, channels: Option<u32>, duration_secs: Option<f64>| {
            let mut job = valid_job();
            job.src_path = feed.to_string_lossy().to_string();
            job.out_dir = dir.0.to_string_lossy().to_string();
            job.mode = mode.into();
            job.format = format.into();
            let probes = ConversionProbes {
                channels,
                channel_gains: None,
                proc: Vec::new(),
                fade_out: None,
                leading_trim: None,
                probed: manifest::ProbedSource {
                    channels,
                    ..manifest::ProbedSource::default()
                },
                duration_secs,
                sgmca_source: None,
                source_samples: None,
            };
            let mut cleanup = OutputCleanup::default();
            plan_conversion(&job, &AudioFeed::plain(feed.to_path_buf()), &probes, &mut cleanup).unwrap()
        };
        let has_rf64 = |plan: &ConversionPlan| plan.encoder_args.windows(2).any(|pair| pair == ["-rf64", "auto"]);

        // A three-hour 4-channel deposition mixed to stereo is about 2 GiB.
        let stereo = plan_for("stereo", "wav", Some(4), Some(3.0 * 3600.0));
        assert!(!has_rf64(&stereo), "{:?}", stereo.encoder_args);
        assert_eq!(stereo.map_args, vec![vec!["-map", "0:a:0"]]);
        // Split writes one mono file per channel.
        assert!(!has_rf64(&plan_for("split", "wav", Some(4), Some(8.0 * 3600.0))));
        // Keep Original with 16 channels passes 3.5 GiB after about 41 minutes.
        let keep_short = plan_for("keep", "wav", Some(16), Some(30.0 * 60.0));
        assert!(!has_rf64(&keep_short));
        assert_eq!(keep_short.map_args, vec![vec!["-map", "0:a:0"]]);
        assert!(has_rf64(&plan_for("keep", "wav", Some(16), Some(50.0 * 60.0))));
        // Unknown duration or channel count keeps the 4 GiB safety.
        assert!(has_rf64(&plan_for("keep", "wav", None, Some(60.0))));
        assert!(has_rf64(&plan_for("stereo", "wav", Some(2), None)));
        // Never for other formats.
        assert!(!has_rf64(&plan_for("keep", "flac", Some(16), Some(50.0 * 60.0))));
        // The -map in the command is the one the record carries.
        let keep_long = plan_for("keep", "wav", Some(16), Some(50.0 * 60.0));
        let rf64 = keep_long.args.iter().position(|arg| arg == "-rf64").unwrap();
        let map = keep_long.args.iter().position(|arg| arg == "-map").unwrap();
        assert_eq!(keep_long.args[map + 1], "0:a:0");
        assert!(map < rf64);
    }

    #[test]
    fn a_wav_output_without_a_riff_or_rf64_header_is_not_delivered() {
        let dir = ConversionTestDir::new();
        let source = dir.0.join("session.wav");
        fs::write(&source, b"source bytes").unwrap();
        let mut job = valid_job();
        job.src_path = source.to_string_lossy().to_string();
        job.mode = "keep".into();
        let probes = ConversionProbes {
            channels: None,
            channel_gains: None,
            proc: Vec::new(),
            fade_out: None,
            leading_trim: None,
            probed: manifest::ProbedSource::default(),
            duration_secs: None,
            sgmca_source: None,
            source_samples: None,
        };
        let mut cleanup = OutputCleanup::default();
        let plan = plan_conversion(&job, &AudioFeed::plain(source.to_path_buf()), &probes, &mut cleanup).unwrap();
        let output = plan.planned[0].path.clone();
        let record = plan.planned[0].manifest.clone();
        fs::write(&output, b"not a wav header").unwrap();
        let input = ManifestInput {
            app_version: "1.1.0".into(),
            created_at: "2026-11-13T09:00:00Z".into(),
            source: FileRecord {
                file_name: "session.wav".into(),
                sha256: "a".repeat(64),
                size_bytes: 12,
                trm: None,
                trm_inspection_error: None,
                sgmca: None,
                dcr: None,
                dm_timeline: None,
            },
            probed: &probes.probed,
            labels: &job.labels,
        };
        let cancel = ConversionCancel::new(Arc::new(AtomicU64::new(1)), 1);
        let error = finish_outputs(&job, &plan, &input, cleanup, &cancel).unwrap_err();
        assert!(error.contains("RIFF or RF64"), "{error}");
        assert!(
            !output.exists() && !record.exists(),
            "the output and its record are removed"
        );
        assert!(source.exists());
    }

    /// FFmpeg-backed: when the container outlasts the converted stream (here a
    /// 4 s a:0 and a 10 s a:1, so format.duration is 10 s), the fade-out is
    /// placed on a:0's own end and the export record holds that filter.
    #[test]
    fn fade_out_is_placed_on_the_converted_streams_end() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let source = dir.0.join("synthetic_short_first_track.mkv");
        run_ffmpeg(
            &[
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=4:sample_rate=48000",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=660:duration=10:sample_rate=48000",
                "-map",
                "0:a",
                "-map",
                "1:a",
                "-c:a",
                "pcm_s16le",
                "-y",
            ]
            .map(String::from)
            .into_iter()
            .chain([source.to_string_lossy().to_string()])
            .collect::<Vec<_>>(),
        );
        let container = ffprobe_json(&source, &["-show_format"]);
        let container: f64 = container["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!(container > 9.9, "fixture: container lasts {container}");

        let mut job = court_job(&source, &dir.0, "keep", "wav");
        job.fade = true;
        job.fade_dur = 2.0;
        job.chan_vols = vec![1.0];
        let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
        let output = &conversion.files[0];
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(manifest::manifest_path_for(Path::new(&output.path))).unwrap()).unwrap();
        assert_eq!(
            record["processing"]["filterChain"],
            "asetpts=N/SR/TB,afade=t=out:st=2.000:d=2,afade=t=in:d=2"
        );
        assert_eq!(record["processing"]["mapArgs"], serde_json::json!(["-map", "0:a:0"]));
        assert_eq!(record["processing"]["fadeOut"]["startSeconds"], 2.0);
        assert_eq!(record["processing"]["fadeOut"]["decodedSourceSeconds"], 4.0);
        assert_eq!(record["processing"]["fadeOut"]["statedSourceSeconds"], 4.0);
        // The last 0.3 s is near the end of the fade-out, so it is much quieter
        // than the tone just before the fade-out starts.
        let middle = max_volume_db_between(Path::new(&output.path), 1.9, 2.0);
        let tail = max_volume_db_between(Path::new(&output.path), 3.7, 4.0);
        assert!(tail < middle - 10.0, "middle {middle} dB, tail {tail} dB");
    }

    /// FFmpeg-backed: a chained Ogg Opus file (a 3 s link, then a 2 s link)
    /// states only its last link's length, 2 s, but decodes to 5 s. The
    /// fade-out is placed from the decoded 5 s, so the first link plays at
    /// full level and only the last 0.5 s fades. Placed from the stated
    /// length it started at 1.5 s and silenced everything after 2 s. The
    /// record names both lengths.
    #[test]
    fn fade_out_is_placed_on_the_decoded_end_when_the_stated_length_is_short() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let mut chained = Vec::new();
        for (index, (frequency, seconds)) in [(440, 3), (660, 2)].into_iter().enumerate() {
            let link = dir.0.join(format!("link_{index}.opus"));
            run_ffmpeg(&[
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                format!("sine=frequency={frequency}:duration={seconds}:sample_rate=48000"),
                "-c:a".into(),
                "libopus".into(),
                "-b:a".into(),
                "64k".into(),
                "-y".into(),
                link.to_string_lossy().to_string(),
            ]);
            chained.extend(fs::read(&link).unwrap());
            fs::remove_file(&link).unwrap();
        }
        let source = dir.0.join("synthetic_chained.opus");
        fs::write(&source, chained).unwrap();
        let stated = ffprobe_json(&source, &["-show_format"]);
        let stated: f64 = stated["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!(
            stated < 2.1,
            "fixture: FFmpeg states {stated} s, not the 5 s it decodes"
        );

        for (mode, format) in [("keep", "wav"), ("stereo", "flac"), ("split", "sgmca")] {
            let out = dir.0.join(format!("out_{mode}"));
            fs::create_dir(&out).unwrap();
            let mut job = court_job(&source, &out, mode, format);
            job.fade = true;
            job.fade_dur = 0.5;
            job.labels.truncate(1);
            job.chan_vols = vec![1.0];
            let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
            let output = &conversion.files[0];
            // A mono source, so not `checked_export_record` (4 channels).
            let record: serde_json::Value =
                serde_json::from_slice(&fs::read(manifest::manifest_path_for(Path::new(&output.path))).unwrap())
                    .unwrap();
            let errors = manifest::tests::schema_errors_for_value(&record);
            assert!(errors.is_empty(), "{mode} {format}: {errors:#?}");
            let fade = &record["processing"]["fadeOut"];
            assert_eq!(fade["decodedSourceSeconds"], 5.0, "{mode} {format}: {fade}");
            assert_eq!(fade["startSeconds"], 4.5, "{mode} {format}");
            assert_eq!(fade["durationSeconds"], 0.5, "{mode} {format}");
            assert!(fade["statedSourceSeconds"].as_f64().unwrap() < 2.1, "{fade}");
            assert!(
                record["processing"]["filterChain"]
                    .as_str()
                    .unwrap()
                    .contains("afade=t=out:st=4.500:d=0.5"),
                "{mode} {format}: {}",
                record["processing"]["filterChain"]
            );
            let path = Path::new(&output.path);
            // Both links are there at the tone's level (lavfi's sine peaks
            // at -18 dBFS); only the end fades.
            for (start, end) in [(1.0, 1.5), (2.5, 3.0), (3.5, 4.4)] {
                let level = max_volume_db_between(path, start, end);
                assert!(level > -21.0, "{mode} {format}: {start}-{end} s at {level} dB");
            }
            let tail = max_volume_db_between(path, 4.9, 5.0);
            let before = max_volume_db_between(path, 4.0, 4.4);
            assert!(
                tail < before - 10.0,
                "{mode} {format}: tail {tail} dB, before {before} dB"
            );
        }
    }

    /// FFmpeg-backed: Keep Original refuses MP3, M4A and Opus for a
    /// 4-channel source before writing anything, because each would fold or
    /// filter channels (MP3 into stereo; AAC and Opus code the fourth channel
    /// of hound's default 3.1 layout as a low-pass LFE). WAV, FLAC and SGMCA
    /// keep every channel at its level.
    #[test]
    fn keep_original_refuses_lossy_formats_that_would_lose_channels() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let source = dir.0.join("synthetic_four_channel.wav");
        // One 1 kHz tone per channel, each in its own second; hound marks
        // four channels as 3.1, as the finding's source was.
        let spec = hound::WavSpec {
            channels: 4,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&source, spec).unwrap();
        for n in 0..4 * 48_000_usize {
            let value = (0.35 * (2.0 * std::f64::consts::PI * 1000.0 * n as f64 / 48_000.0).sin() * 32767.0) as i16;
            for channel in 0..4 {
                writer
                    .write_sample(if n / 48_000 == channel { value } else { 0 })
                    .unwrap();
            }
        }
        writer.finalize().unwrap();
        let layout = ffprobe_json(&source, &["-show_streams"]);
        assert_eq!(layout["streams"][0]["channel_layout"], "3.1", "fixture layout");

        for format in ["mp3", "m4a", "opus"] {
            let out = dir.0.join(format!("out_{format}"));
            fs::create_dir(&out).unwrap();
            let job = court_job(&source, &out, "keep", format);
            let error = convert_with_system_ffmpeg(&job, None).expect_err("refused");
            assert!(error.contains("4-channel"), "{format}: {error}");
            assert!(error.contains("WAV, FLAC or SGMCA"), "{format}: {error}");
            assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "{format}: nothing written");
        }
        for format in ["wav", "flac", "sgmca"] {
            let out = dir.0.join(format!("out_{format}"));
            fs::create_dir(&out).unwrap();
            let job = court_job(&source, &out, "keep", format);
            let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
            let path = Path::new(&conversion.files[0].path);
            let record = checked_export_record(&conversion.files[0], &source);
            assert_eq!(record["channel"]["output"], "all-channels");
            // Every output channel carries its own second's tone at the
            // source level (-9 dBFS) and nothing in the other seconds.
            let decoded = if format == "sgmca" {
                decode_f32(path, 4)
            } else {
                decode_f32_default(path, 4)
            };
            for (channel, samples) in decoded.iter().enumerate() {
                for second in 0..4 {
                    let window = &samples[second * 48_000 + 4_800..second * 48_000 + 43_200];
                    let peak = window.iter().fold(0.0_f32, |peak, value| peak.max(value.abs()));
                    if second == channel {
                        assert!((0.3..0.4).contains(&peak), "{format} channel {channel}: peak {peak}");
                    } else {
                        assert!(peak < 0.01, "{format} channel {channel} second {second}: {peak}");
                    }
                }
            }
        }
    }

    /// FFmpeg-backed: a file whose audio streams differ in channel count is
    /// converted from stream 0 in every mode, the export record says so, and
    /// the queue probe reads both tracks with their tags.
    #[test]
    fn synthetic_two_stream_file_converts_stream_zero_and_records_it() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let source = synthetic_two_stream_mkv(&dir.0);

        // Control: FFmpeg's own choice, with no map, is the 4-channel stream.
        let control = dir.0.join("control_auto.wav");
        run_ffmpeg(&[
            "-i".into(),
            source.to_string_lossy().to_string(),
            "-c:a".into(),
            "pcm_s16le".into(),
            control.to_string_lossy().to_string(),
        ]);
        assert_audio(&control, 4, "48000");

        let streams = {
            let output = std::process::Command::new("ffprobe")
                .args(crate::ffmpeg::manifest_probe_args(&source))
                .output()
                .unwrap();
            manifest::parse_audio_streams(&serde_json::from_slice(&output.stdout).unwrap())
        };
        assert_eq!(streams.converted_index, 0);
        assert_eq!(streams.streams.len(), 2);
        assert_eq!(streams.streams[0].channels, Some(1));
        assert_eq!(streams.streams[0].language.as_deref(), Some("eng"));
        assert_eq!(streams.streams[0].title.as_deref(), Some("Floor"));
        assert_eq!(streams.streams[1].channels, Some(4));
        assert_eq!(streams.streams[1].title.as_deref(), Some("Interpreter"));

        // Stream 0 peaks at 0.5 x 1/8 of full scale; stream 1 is 18 dB lower.
        let floor_peak = 20.0 * (0.5_f64 / 8.0).log10();
        for (mode, outputs) in [("keep", 1), ("stereo", 1), ("split", 1)] {
            let out = dir.0.join(mode);
            fs::create_dir(&out).unwrap();
            let mut job = court_job(&source, &out, mode, "wav");
            job.labels = vec!["Floor".into()];
            job.chan_vols = vec![1.0];
            let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
            assert_eq!(conversion.files.len(), outputs, "{mode}");
            let file = &conversion.files[0];
            let path = Path::new(&file.path);
            assert_audio(path, if mode == "stereo" { 2 } else { 1 }, "48000");
            let peak = max_volume_db(path);
            assert!(
                (peak - floor_peak).abs() < 1.5,
                "{mode}: peak {peak} dB, expected stream 0 at {floor_peak:.1} dB"
            );
            assert_records_stream_zero_of_two(file, &source);
        }
    }

    #[test]
    fn synthetic_sgmca_split_applies_each_channels_own_auto_level_gain() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (sgmca, _) = synthetic_sgmca(&dir.0);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let mut job = court_job(&sgmca, &out, "split", "wav");
        job.auto_level = true;
        // Distinct gains, so a gain that landed on the wrong output (or not at
        // all) moves that output's level by 6 dB or more.
        let gains = [1.0, 4.0, 0.5, 8.0];

        let conversion = convert_with_system_ffmpeg(&job, Some(gains.to_vec())).unwrap();
        assert_eq!(conversion.files.len(), 4);
        for (index, file) in conversion.files.iter().enumerate() {
            let path = Path::new(&file.path);
            assert_audio(path, 1, "48000");
            let expected = 20.0 * (SYNTHETIC_CHANNEL_VOLUMES[index] * gains[index] / 8.0).log10();
            let peak = max_volume_db(path);
            assert!(
                (peak - expected).abs() < 1.5,
                "channel {index}: peak {peak} dB, expected {expected:.1} dB with gain {}",
                gains[index]
            );
            let record = checked_export_record(file, &sgmca);
            assert_eq!(record["channel"]["sourceChannelIndex"], index);
        }
        let record = checked_export_record(&conversion.files[0], &sgmca);
        let chain = record["processing"]["filterChain"].as_str().unwrap();
        assert!(chain.contains("[sp1]pan=mono|c0=c1,volume=4.0000[op1]"), "{chain}");
        assert!(chain.contains("[sp3]pan=mono|c0=c3,volume=8.0000[op3]"), "{chain}");
    }

    #[test]
    fn synthetic_sgmca_mix_applies_auto_level_gains_to_the_right_channels() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (sgmca, _) = synthetic_sgmca(&dir.0);

        // A mix sums every channel into one signal, so per-channel levels
        // cannot be read back from it. Keep only source channel 2, at gain 2
        // (the pan weight and volume make each gain a plain factor): the mix
        // then peaks at about -30 dBFS; the same gain on channel 0, 1 or 3
        // would give about -18, -24 or -36 dBFS, and an ignored vector (an
        // equal mix of all four) at least about -26 dBFS (its RMS).
        let out = dir.0.join("gains");
        fs::create_dir(&out).unwrap();
        let mut job = court_job(&sgmca, &out, "stereo", "wav");
        job.auto_level = true;
        let conversion = convert_with_system_ffmpeg(&job, Some(vec![0.0, 0.0, 2.0, 0.0])).unwrap();
        assert_mix_output(&conversion, &sgmca, "synthetic_session.wav");
        let record = checked_export_record(&conversion.files[0], &sgmca);
        let chain = record["processing"]["filterChain"].as_str().unwrap();
        assert!(chain.contains("0.0000*c1+0.5000*c2+0.0000*c3"), "{chain}");
        let expected = 20.0 * (SYNTHETIC_CHANNEL_VOLUMES[2] * 2.0 / 8.0).log10();
        let peak = max_volume_db(Path::new(&conversion.files[0].path));
        assert!(
            (peak - expected).abs() < 1.5,
            "mix peak {peak} dB, expected {expected:.1} dB"
        );

        // The channel-count guard: a vector for 3 channels must not be applied
        // to a 4-channel source, so the mix falls back to all four channels.
        let guarded = dir.0.join("guarded");
        fs::create_dir(&guarded).unwrap();
        let mut job = court_job(&sgmca, &guarded, "stereo", "wav");
        job.auto_level = true;
        let conversion = convert_with_system_ffmpeg(&job, Some(vec![0.0, 0.0, 0.0])).unwrap();
        assert_mix_output(&conversion, &sgmca, "synthetic_session.wav");
        let record = checked_export_record(&conversion.files[0], &sgmca);
        let chain = record["processing"]["filterChain"].as_str().unwrap();
        assert!(chain.contains("0.2500*c0+0.2500*c1+0.2500*c2+0.2500*c3"), "{chain}");
        let peak = max_volume_db(Path::new(&conversion.files[0].path));
        assert!(peak > -27.0, "a 3-channel gain vector was applied: mix peak {peak} dB");
    }

    #[test]
    fn synthetic_sgmca_split_with_loudness_normalize_and_fades_stays_valid() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (sgmca, _) = synthetic_sgmca(&dir.0);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let mut job = court_job(&sgmca, &out, "split", "wav");
        job.normalize = true;
        job.fade = true;
        job.fade_dur = 0.5;

        let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
        assert_eq!(conversion.files.len(), 4);
        let mut peaks = Vec::new();
        for (index, file) in conversion.files.iter().enumerate() {
            let path = Path::new(&file.path);
            // loudnorm works at 192 kHz; the encoder arguments bring it back.
            assert_audio(path, 1, "48000");
            let middle = max_volume_db_between(path, 0.8, 1.2);
            let start = max_volume_db_between(path, 0.0, 0.05);
            let end = max_volume_db_between(path, 1.93, 2.0);
            assert!(
                start < middle - 10.0,
                "channel {index}: no fade-in ({start} vs {middle} dB)"
            );
            assert!(
                end < middle - 10.0,
                "channel {index}: no fade-out ({end} vs {middle} dB)"
            );
            peaks.push(middle);

            let record = checked_export_record(file, &sgmca);
            assert_eq!(record["channel"]["sourceChannelIndex"], index);
            let chain = record["processing"]["filterChain"].as_str().unwrap();
            // The fade-out start comes from the probed duration (Ogg Opus
            // reports a few ms over 2 s), so only its presence is checked here;
            // the levels above check where it landed.
            for filter in ["loudnorm=I=", "afade=t=out:st=", "afade=t=in:d=0.5"] {
                assert_eq!(chain.matches(filter).count(), 4, "{filter} once per channel: {chain}");
            }
        }
        // The source channels are 18 dB apart; normalized, they are close.
        let spread = peaks.iter().cloned().fold(f64::MIN, f64::max) - peaks.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            spread < 6.0,
            "normalized channels still {spread:.1} dB apart: {peaks:?}"
        );
    }

    // ── SGMCA layout, writer and slicing (FFmpeg on PATH) ───────────────────

    /// The granule-based duration must match what libopus actually decodes,
    /// sample for sample, and equal `expected` exactly, so a reader that
    /// loses or invents audio fails here.
    fn assert_sgmca_samples(path: &Path, info: &crate::sgmca::SgmcaInfo, expected: u64) {
        let channels = usize::from(info.head.channels);
        let decoded = decode_f32(path, channels);
        assert!(
            decoded.iter().all(|channel| channel.len() as u64 == info.samples),
            "libopus decoded {:?} samples per channel, the granules say {}",
            decoded.iter().map(Vec::len).collect::<Vec<_>>(),
            info.samples
        );
        assert_eq!(info.samples, expected, "{info:?}");
    }

    /// `assert_sgmca_samples` for a synthetic fixture: exactly
    /// `SYNTHETIC_SECONDS` at 48 kHz (every fixture is whole 20 ms frames).
    fn assert_sgmca_duration(path: &Path, info: &crate::sgmca::SgmcaInfo) {
        assert_sgmca_samples(path, info, (SYNTHETIC_SECONDS * 48_000.0) as u64);
    }

    /// An SGMCA output of `source` holds exactly the samples FFmpeg decodes
    /// from the source at 48 kHz, counted by decoding it, not assumed.
    fn assert_sgmca_holds_source(path: &Path, info: &crate::sgmca::SgmcaInfo, source: &Path) {
        let decoded = decode_f32(source, usize::from(info.head.channels));
        let source_samples = decoded[0].len() as u64;
        assert!(decoded.iter().all(|channel| channel.len() as u64 == source_samples));
        assert_sgmca_samples(path, info, source_samples);
    }

    #[test]
    fn bare_ogg_family_255_sgmca_is_read_in_place_and_split() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let sgmca = synthetic_plain_sgmca(&dir.0);
        let info = crate::sgmca::inspect(&sgmca, None).unwrap();
        assert!(info.sgmca_layout, "{info:?}");
        assert_eq!(
            (info.head.channels, info.head.stream_count, info.head.coupled_count),
            (4, 4, 0)
        );
        assert_eq!(info.head.input_sample_rate, 16_000);
        assert_sgmca_duration(&sgmca, &info);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&sgmca, &out, "split", "wav"), None).unwrap();
        assert_split_outputs(&conversion, &sgmca, ".wav");
        assert_eq!(conversion.feed, sgmca, "a bare Ogg SGMCA is read in place");
        let record = checked_export_record(&conversion.files[0], &sgmca);
        assert_eq!(record["source"]["sgmca"]["legacyBareOgg"], true);
        assert_eq!(record["source"]["sgmca"]["fileVersion"], serde_json::Value::Null);
        assert_eq!(
            record["source"]["sgmca"]["segments"][0]["startText"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn wav_to_sgmca_writes_one_mono_opus_stream_per_channel_with_a_record() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let wav = dir.0.join("synthetic_session.wav");
        synthetic_four_channel(&wav, 48_000, &["-c:a", "pcm_s16le", "-f", "wav"]);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&wav, &out, "keep", "sgmca"), None).unwrap();
        assert_eq!(conversion.files.len(), 1);
        let file = &conversion.files[0];
        assert_eq!(file.name, "synthetic_session_orig.sgmca");
        let written = Path::new(&file.path);
        let info = crate::sgmca::verify_written(written, None, None).unwrap();
        // The Direct Audio layout, byte for byte: file header, one segment
        // header with the local start and the exact payload length, then the
        // Ogg stream.
        let bytes = fs::read(written).unwrap();
        assert_eq!(&bytes[0..8], &[0x28, 0, 0, 0, 1, 0, 0, 0]);
        assert!(bytes[8..24].iter().any(|&byte| byte != 0));
        assert_eq!(&bytes[24..40], &[0; 16]);
        assert_eq!(&bytes[40..56], &[0x38, 0, 0, 0, 1, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
        let start = std::str::from_utf8(&bytes[56..80]).unwrap();
        assert!(crate::sgmca::parse_start_text(start).is_some(), "{start}");
        assert_eq!(&bytes[80..88], &[0; 8]);
        assert_eq!(
            u64::from_le_bytes(bytes[88..96].try_into().unwrap()),
            bytes.len() as u64 - 96
        );
        assert_eq!(&bytes[96..100], b"OggS");
        assert_eq!(info.segments.len(), 1);
        assert_eq!(info.head.channels, 4);
        assert_eq!(info.head.mapping, [0, 1, 2, 3]);
        assert_eq!(info.head.input_sample_rate, 48_000);
        // Every sample of the source, and the source is the whole fixture.
        assert_sgmca_holds_source(written, &info, &wav);
        assert_sgmca_duration(written, &info);
        assert_audio(written, 4, "48000");
        let record = checked_export_record(file, &wav);
        assert_eq!(record["output"]["format"], "sgmca");
        assert_eq!(record["output"]["sampleRate"], 48_000);
        // No source start, so the segment start is the conversion time, and
        // the record says so.
        let layout = &record["output"]["sgmca"];
        assert_eq!(layout["startSource"], "conversion-time");
        assert!(layout["startNote"]
            .as_str()
            .unwrap()
            .contains("not when the recording was made"));
        assert_eq!(layout["segments"][0]["startText"], start);
        assert_eq!(layout["segments"][0]["mappingFamily"], 255);
        assert!(record["source"].get("sgmca").is_none());
        // 32 kbps for each of the four mono streams.
        let encoder: Vec<String> = serde_json::from_value(record["processing"]["encoderArgs"].clone()).unwrap();
        assert!(encoder.windows(2).any(|pair| pair == ["-b:a", "128k"]), "{encoder:?}");

        // What DepoAudio writes, DepoAudio reads back: split it to WAV and
        // every channel comes out where it went in, at its own level.
        let round = dir.0.join("round");
        fs::create_dir(&round).unwrap();
        let source = round.join("synthetic_session.sgmca");
        fs::copy(written, &source).unwrap();
        let out_wav = round.join("out");
        fs::create_dir(&out_wav).unwrap();
        let split = convert_with_system_ffmpeg(&court_job(&source, &out_wav, "split", "wav"), None).unwrap();
        assert_split_outputs(&split, &source, ".wav");
    }

    #[test]
    fn split_to_sgmca_gives_each_channel_its_own_direct_audio_file() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (sgmca, _) = synthetic_sgmca(&dir.0);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&sgmca, &out, "split", "sgmca"), None).unwrap();
        assert_split_outputs(&conversion, &sgmca, ".sgmca");
        for file in &conversion.files {
            let info = crate::sgmca::verify_written(Path::new(&file.path), None, None).unwrap();
            assert_eq!((info.head.channels, info.head.stream_count), (1, 1));
            // The source's segment start is kept.
            let start = info.segments[0].start.as_ref().unwrap();
            assert_eq!(start.text, "2026-11-22T10:15:30-0500");
            let record = checked_export_record(file, &sgmca);
            assert_eq!(record["output"]["sgmca"]["startSource"], "sgmca-segment-start");
        }
    }

    #[test]
    fn sgmca_slice_is_sample_exact_and_matches_the_original_audio() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let (sgmca, _) = synthetic_sgmca(&dir.0);
        let cut = dir.0.join("cut.sgmca");
        // Cut exactly where asked, 0.5 s to 1.5 s, even though 0.5 s is not
        // a whole second: no audio is left out to make the start round.
        let report = crate::sgmca::slice(&sgmca, &cut, 0.5, Some(1.5), None).unwrap();
        assert_eq!(report.samples, 48_000);
        assert_eq!(report.start_seconds, 0.5);
        assert_eq!(report.channels, 4);
        assert_eq!(report.start_offset_ms, Some(500.0));
        assert!(report
            .start_note
            .contains("this recording's first sample is 0.5 s after the time written"));

        // FFmpeg honours the new pre-skip and end trim: exactly one second.
        let sliced = decode_f32(&cut, 4);
        let whole = decode_f32(&sgmca, 4);
        assert!(
            sliced.iter().all(|channel| channel.len() == 48_000),
            "{}",
            sliced[0].len()
        );
        // The copied packets decode to the same audio as the same second of
        // the full recording; the pre-roll lets the decoder converge first.
        for (channel, (cut, full)) in sliced.iter().zip(&whole).enumerate() {
            let window = &full[24_000..72_000];
            let signal: f64 = window.iter().map(|&s| (s as f64).powi(2)).sum();
            let error: f64 = cut.iter().zip(window).map(|(&a, &b)| ((a - b) as f64).powi(2)).sum();
            let ratio_db = 10.0 * (error / signal).log10();
            assert!(ratio_db < -30.0, "channel {channel}: slice differs by {ratio_db:.1} dB");
        }
        // And the slice is a valid Direct Audio file in its own right, whose
        // start is the session's start plus the whole seconds skipped (none:
        // the whole second at or before 0.5 s is the session's start).
        let start = ["2026-11-22T10:15:30-0500".to_string()];
        assert!(crate::sgmca::verify_written(&cut, Some(&start), None).is_ok());
    }

    /// One 4-channel session of a `frequency` tone lasting `seconds`, encoded
    /// as family 255 (one mono stream per channel). With `family_one`, the
    /// OpusHead mapping family byte is changed to 1 with the same uncoupled
    /// layout, which is what the notes say CATalyst's import writes; FFmpeg's
    /// encoder would couple streams in family 1.
    /// Made at 16 kHz, the encoder's rate, so every session (1.0, 1.5 and
    /// 0.8 s) is a whole number of 20 ms frames whatever the FFmpeg version.
    fn tone_session(dir: &Path, frequency: u32, seconds: f64, family_one: bool) -> Vec<u8> {
        let ogg = dir.join(format!("tone_{frequency}.ogg"));
        run_ffmpeg(&[
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!(
                "sine=frequency={frequency}:duration={seconds}:sample_rate=16000:samples_per_frame={}",
                synthetic_frame(16_000)
            ),
            "-af".into(),
            "pan=4c|c0=c0|c1=c0|c2=c0|c3=c0".into(),
            "-c:a".into(),
            "libopus".into(),
            "-mapping_family".into(),
            "255".into(),
            "-frame_duration".into(),
            "20".into(),
            "-b:a".into(),
            "128k".into(),
            "-ar".into(),
            "16000".into(),
            "-f".into(),
            "ogg".into(),
            "-y".into(),
            ogg.to_string_lossy().to_string(),
        ]);
        let mut bytes = fs::read(&ogg).unwrap();
        fs::remove_file(&ogg).unwrap();
        if family_one {
            // First page: 27-byte header, one lacing value, then OpusHead,
            // whose mapping family is byte 18.
            let page_len = 28 + bytes[27] as usize;
            assert_eq!(&bytes[28..36], b"OpusHead");
            assert_eq!(bytes[28 + 18], 255);
            bytes[28 + 18] = 1;
            bytes[22..26].copy_from_slice(&[0; 4]);
            let crc = crate::sgmca::ogg_crc(&bytes[..page_len]);
            bytes[22..26].copy_from_slice(&crc.to_le_bytes());
        }
        bytes
    }

    const SESSION_TONES: [(u32, f64); 3] = [(300, 1.0), (700, 1.5), (1500, 0.8)];
    const SESSION_STARTS: [&str; 3] = [
        "2026-11-22T10:15:30-0500",
        "2026-11-22T11:02:05-0500",
        "2026-11-22T13:40:00-0500",
    ];

    /// Three recording sessions of different tones (family 255, then family
    /// 1, then 255) in one Direct Audio file.
    fn three_session_sgmca(dir: &Path) -> PathBuf {
        let streams: Vec<Vec<u8>> = SESSION_TONES
            .iter()
            .enumerate()
            .map(|(index, (frequency, seconds))| tone_session(dir, *frequency, *seconds, index == 1))
            .collect();
        let segments: Vec<(&str, &[u8])> = SESSION_STARTS
            .iter()
            .zip(&streams)
            .map(|(start, stream)| (*start, stream.as_slice()))
            .collect();
        let path = dir.join("synthetic_session.sgmca");
        fs::write(&path, crate::sgmca::fixtures::direct_audio(&segments)).unwrap();
        path
    }

    /// The tone frequency of `samples` (48 kHz), from its zero crossings.
    fn tone_frequency(samples: &[f32]) -> f64 {
        let crossings = samples
            .windows(2)
            .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
            .count();
        crossings as f64 / 2.0 / (samples.len() as f64 / 48_000.0)
    }

    /// Every session's tone is in the decoded audio, in file order, for the
    /// whole of its length.
    fn assert_sessions_in_order(decoded: &[f32], boundaries: &[u64]) {
        for (index, (frequency, _)) in SESSION_TONES.iter().enumerate().take(boundaries.len()) {
            let start = boundaries[index] as usize;
            let end = boundaries.get(index + 1).map_or(decoded.len(), |&end| end as usize);
            // Leave 50 ms at each edge for the codec's transition.
            let window = &decoded[start + 2_400..end - 2_400];
            let found = tone_frequency(window);
            assert!(
                (found - f64::from(*frequency)).abs() < f64::from(*frequency) * 0.03,
                "session {}: expected {frequency} Hz, found {found:.0} Hz",
                index + 1
            );
        }
    }

    #[test]
    fn every_recording_session_is_converted_in_file_order() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let sgmca = three_session_sgmca(&dir.0);
        let info = crate::sgmca::inspect(&sgmca, None).unwrap();
        assert_eq!(info.segments.len(), 3);
        assert_eq!(
            info.segments
                .iter()
                .map(|segment| segment.head.mapping_family)
                .collect::<Vec<_>>(),
            [255, 1, 255]
        );
        for (segment, (_, seconds)) in info.segments.iter().zip(SESSION_TONES) {
            assert_eq!(segment.samples, (seconds * 48_000.0) as u64, "{segment:?}");
        }
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&sgmca, &out, "keep", "wav"), None).unwrap();
        let file = &conversion.files[0];
        let decoded = decode_f32(Path::new(&file.path), 4);
        // Exactly the sessions' samples, joined with no gap and nothing lost.
        assert_eq!(decoded[0].len() as u64, info.samples);
        let boundaries: Vec<u64> = info.segments.iter().map(|segment| segment.start_sample).collect();
        for channel in &decoded {
            assert_sessions_in_order(channel, &boundaries);
        }
        let record = checked_export_record(file, &sgmca);
        let segments = record["source"]["sgmca"]["segments"].as_array().unwrap();
        assert_eq!(segments.len(), 3);
        for (segment, (recorded, start)) in info.segments.iter().zip(segments.iter().zip(SESSION_STARTS)) {
            assert_eq!(recorded["startText"], start);
            assert_eq!(recorded["startSample"], segment.start_sample);
            assert_eq!(recorded["mappingFamily"], segment.head.mapping_family);
        }
        assert!(!conversion.feed.exists(), "the concat script was left behind");
    }

    #[test]
    fn a_still_recording_sgmca_converts_its_complete_audio_and_says_so() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        // Two sessions, the second (1.5 s, two Ogg audio pages) cut 300
        // bytes before its end, as a file still being written would be.
        let streams: Vec<Vec<u8>> = SESSION_TONES[..2]
            .iter()
            .map(|(frequency, seconds)| tone_session(&dir.0, *frequency, *seconds, false))
            .collect();
        let bytes =
            crate::sgmca::fixtures::direct_audio(&[(SESSION_STARTS[0], &streams[0]), (SESSION_STARTS[1], &streams[1])]);
        let growing = dir.0.join("synthetic_session.sgmca");
        fs::write(&growing, &bytes[..bytes.len() - 300]).unwrap();
        let info = crate::sgmca::inspect(&growing, None).unwrap();
        assert!(info.growing);
        assert_eq!(info.segments.len(), 2);
        assert!(!info.segments[1].complete);
        assert!(info.segments[1].samples > 0 && info.segments[1].duration_seconds < 1.5);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&growing, &out, "keep", "wav"), None).unwrap();
        let file = &conversion.files[0];
        let decoded = decode_f32(Path::new(&file.path), 4);
        // FFmpeg is handed only the complete pages, and decodes up to the
        // last of them (its last packet may run past that page's granule).
        let decoded_samples = decoded[0].len() as u64;
        assert!(
            decoded_samples >= info.samples && decoded_samples <= info.samples + 960,
            "decoded {decoded_samples}, the reader counts {}",
            info.samples
        );
        let boundaries: Vec<u64> = info.segments.iter().map(|segment| segment.start_sample).collect();
        assert_sessions_in_order(&decoded[0], &boundaries);
        let record = checked_export_record(file, &growing);
        assert_eq!(record["source"]["sgmca"]["growing"], true);
        // The cut-off page is recorded as unread, with the session it is in.
        assert!(info.unread_bytes > 0);
        assert_eq!(record["source"]["sgmca"]["unreadBytes"], info.unread_bytes);
        assert_eq!(record["source"]["sgmca"]["omittedSegments"], serde_json::json!([]));
        let queue = manifest::sgmca_audio_streams(&info);
        assert_eq!(queue.recording_sessions, Some(2));
        assert_eq!(queue.still_recording, Some(true));
        assert_eq!(queue.streams[0].channels, Some(4));
        assert_eq!(queue.cut_short_session, Some(2));
        assert_eq!(queue.unread_bytes, Some(info.unread_bytes));

        let sgmca_out = dir.0.join("sgmca-out");
        fs::create_dir(&sgmca_out).unwrap();
        // To SGMCA, the complete audio passes the sample check: each output
        // session holds exactly the samples the reader counted.
        let conversion = convert_with_system_ffmpeg(&court_job(&growing, &sgmca_out, "keep", "sgmca"), None).unwrap();
        let written = crate::sgmca::verify_written(Path::new(&conversion.files[0].path), None, None).unwrap();
        let samples =
            |info: &crate::sgmca::SgmcaInfo| info.segments.iter().map(|segment| segment.samples).collect::<Vec<_>>();
        assert_eq!(samples(&written), samples(&info));
    }

    #[test]
    fn a_multi_session_sgmca_output_keeps_each_session_and_its_start() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let sgmca = three_session_sgmca(&dir.0);
        let source = crate::sgmca::inspect(&sgmca, None).unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&sgmca, &out, "keep", "sgmca"), None).unwrap();
        let written = Path::new(&conversion.files[0].path);
        let starts: Vec<String> = SESSION_STARTS.iter().map(|start| start.to_string()).collect();
        let info = crate::sgmca::verify_written(written, Some(&starts), None).unwrap();
        assert_eq!(info.segments.len(), 3);
        // Each output session starts where the source's did and holds
        // exactly the source session's samples (the check after writing
        // refuses anything else).
        for (output, input) in info.segments.iter().zip(&source.segments) {
            assert_eq!(output.start_sample, input.start_sample);
            assert_eq!(output.samples, input.samples);
        }
        assert_eq!(info.samples, source.samples);
        // Decoded through the reader, the tones come back in order.
        let decoded = decode_f32(written, 4);
        let boundaries: Vec<u64> = info.segments.iter().map(|segment| segment.start_sample).collect();
        assert_sessions_in_order(&decoded[0], &boundaries);

        let record = checked_export_record(&conversion.files[0], &sgmca);
        let layout = &record["output"]["sgmca"];
        assert_eq!(layout["startSource"], "sgmca-segment-start");
        assert!(layout["startNote"]
            .as_str()
            .unwrap()
            .contains("Each of the 3 recording sessions"));
        assert_eq!(layout["segments"].as_array().unwrap().len(), 3);
        assert_eq!(layout["segments"][2]["startText"], SESSION_STARTS[2]);

        // Leading-silence trim would move every session's place and start
        // time: it is refused on a multi-session source, whatever the output
        // format and even when the recording starts with sound, before
        // anything is written.
        for format in ["sgmca", "wav"] {
            let trimmed_out = dir.0.join(format!("trimmed-{format}"));
            fs::create_dir(&trimmed_out).unwrap();
            let mut job = court_job(&sgmca, &trimmed_out, "keep", format);
            job.trim = true;
            let error = convert_with_system_ffmpeg(&job, None).unwrap_err();
            assert!(
                error.starts_with("Trim Leading Silence cannot be used on this file: it holds 3 recording sessions"),
                "{error}"
            );
            assert!(error.contains("Nothing was converted."), "{error}");
            assert_eq!(
                fs::read_dir(&trimmed_out).unwrap().count(),
                0,
                "no output or placeholder is left"
            );
        }
    }

    #[test]
    fn a_bwf_source_dates_its_sgmca_output_from_the_bext_chunk() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let wav = dir.0.join("synthetic_session.wav");
        // 10:15:30.25 after midnight at 48 kHz.
        let reference = (10 * 3600 + 15 * 60 + 30) * 48_000 + 12_000;
        synthetic_four_channel(
            &wav,
            48_000,
            &[
                "-c:a",
                "pcm_s16le",
                "-write_bext",
                "1",
                "-metadata",
                &format!("time_reference={reference}"),
                "-metadata",
                "origination_date=2026-11-22",
                "-f",
                "wav",
            ],
        );
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&court_job(&wav, &out, "keep", "sgmca"), None).unwrap();
        let path = Path::new(&conversion.files[0].path);
        let info = crate::sgmca::verify_written(path, None, None).unwrap();
        // The header holds whole seconds, so it names 10:15:30, the second
        // at or before the first sample, and every source sample is kept:
        // the output holds exactly the source's 2 s at 48 kHz.
        let start = info.segments[0].start.as_ref().unwrap();
        assert_eq!(start.local.as_deref(), Some("2026-11-22T10:15:30"), "{start:?}");
        assert_eq!(info.segments[0].start_sample, 0);
        assert_sgmca_holds_source(path, &info, &wav);
        assert_sgmca_duration(path, &info);
        let path_record = manifest::manifest_path_for(path);
        let record: serde_json::Value = serde_json::from_slice(&fs::read(path_record).unwrap()).unwrap();
        assert!(manifest::tests::schema_errors_for_value(&record).is_empty());
        assert_eq!(record["output"]["sgmca"]["startSource"], "bwf-bext");
        assert_eq!(record["output"]["sgmca"]["startOffsetMs"], 250.0);
        let note = record["output"]["sgmca"]["startNote"].as_str().unwrap();
        assert!(
            note.contains(
                "CATalyst stores whole seconds; this recording's first sample is 0.25 s after the time written."
            ),
            "{note}"
        );
        assert!(note.contains("Every sample of the source is kept."), "{note}");
    }

    #[test]
    fn sgmca_output_start_plans_prefer_the_source_then_bwf_then_the_clock() {
        let now = time::macros::datetime!(2026-11-22 18:00:00 UTC);
        let probed = manifest::ProbedSource::default();
        // A source counted at 32,000 samples at 16 kHz must give 96,000.
        const LENGTH: Option<(u64, u32)> = Some((32_000, 16_000));
        let clock = plan_sgmca_output(None, LENGTH, &probed, false, now).unwrap();
        assert_eq!(clock.start_source, "conversion-time");
        assert_eq!(
            clock.expected_samples,
            Some(ExpectedSamples {
                total: 96_000,
                segments: vec![96_000],
                rounding: 0
            })
        );
        // With no length known an SGMCA output cannot be checked, so it is
        // not made; after leading-silence trim nothing exact is known and
        // none is asked for.
        assert_eq!(
            plan_sgmca_output(None, None, &probed, false, now),
            Err(SOURCE_LENGTH_UNKNOWN_MESSAGE.to_string())
        );
        assert_eq!(
            plan_sgmca_output(None, None, &probed, true, now)
                .unwrap()
                .expected_samples,
            None
        );
        assert_eq!(clock.segments.len(), 1);
        assert_eq!(
            clock.segments[0].start_text,
            crate::clock::local_start_text(now).unwrap()
        );

        let timed = manifest::ProbedSource {
            timing: Some(manifest::SourceTiming {
                time_reference_samples: 3600 * 16_000 + 8_000,
                sample_rate: 16_000,
                source: manifest::BEXT_TIMING_SOURCE,
            }),
            ..manifest::ProbedSource::default()
        };
        let undated = plan_sgmca_output(None, LENGTH, &timed, false, now).unwrap();
        assert_eq!(undated.start_source, "conversion-time");
        assert!(undated.note.contains("no usable origination date"));
        let dated = manifest::ProbedSource {
            origination_date: Some("2026-11-22".into()),
            ..timed.clone()
        };
        // 01:00:00.5: the header holds whole seconds, so it names 01:00:00,
        // the second at or before the first sample; every sample is kept
        // and the half second is recorded.
        let bwf = plan_sgmca_output(None, LENGTH, &dated, false, now).unwrap();
        assert_eq!(bwf.start_source, "bwf-bext");
        assert!(bwf.segments[0].start_text.starts_with("2026-11-22T01:00:00"));
        assert_eq!(bwf.segments.len(), 1);
        assert_eq!(bwf.segments[0].start_sample, 0);
        assert_eq!(bwf.start_offset_ms, Some(500.0));
        assert!(
            bwf.note.contains("first sample is 0.5 s after the time written"),
            "{}",
            bwf.note
        );
        assert!(!bwf.note.contains("left out"));
        // On a whole second nothing is cut.
        let on_second = manifest::ProbedSource {
            timing: Some(manifest::SourceTiming {
                time_reference_samples: 3600 * 16_000,
                sample_rate: 16_000,
                source: manifest::BEXT_TIMING_SOURCE,
            }),
            ..dated.clone()
        };
        let exact = plan_sgmca_output(None, LENGTH, &on_second, false, now).unwrap();
        assert!(exact.segments[0].start_text.starts_with("2026-11-22T01:00:00"));
        assert_eq!(exact.segments[0].start_sample, 0);
        assert_eq!(exact.start_offset_ms, Some(0.0));
        assert!(exact.note.contains("falls on a whole second"));
        assert_eq!(clock.start_offset_ms, None);
        // One source sample short of a second at 44.1 kHz: still the second
        // before (floor), with the fraction recorded and nothing cut.
        let odd = bwf_start_text(
            manifest::SourceTiming {
                time_reference_samples: 44_100 * 7 + 1,
                sample_rate: 44_100,
                source: manifest::BEXT_TIMING_SOURCE,
            },
            "2026-11-22",
        )
        .unwrap();
        assert!(odd.text.starts_with("2026-11-22T00:00:07"), "{odd:?}");
        assert_eq!(odd.offset_ms, 0.023);
        let late = bwf_start_text(
            manifest::SourceTiming {
                time_reference_samples: 44_100 * 8 - 1,
                sample_rate: 44_100,
                source: manifest::BEXT_TIMING_SOURCE,
            },
            "2026-11-22",
        )
        .unwrap();
        assert!(late.text.starts_with("2026-11-22T00:00:07"), "{late:?}");
        assert_eq!(late.offset_ms, 999.977);
        // Trim moves the start, so even a dated source gets the clock.
        assert_eq!(
            plan_sgmca_output(None, None, &dated, true, now).unwrap().start_source,
            "conversion-time"
        );
    }

    #[test]
    fn vendor_notes_two_session_sample_decodes_each_tone_in_its_place() {
        if !court_fixture_tools_available() {
            return;
        }
        // Synthetic tones wrapped by the owner's tool, not by CATalyst (see
        // scripts/fixtures/sgmca/vendor-notes-samples/README.md). Converted
        // from a copy so the committed file is never an input path.
        let dir = ConversionTestDir::new();
        let sgmca = dir.0.join("tone_2sessions.sgmca");
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/fixtures/sgmca/vendor-notes-samples/tone_2sessions.sgmca"),
            &sgmca,
        )
        .unwrap();
        let original = fs::read(&sgmca).unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let mut job = court_job(&sgmca, &out, "keep", "wav");
        job.labels = ["Left", "Right"].map(String::from).to_vec();
        job.chan_vols = vec![1.0; 2];
        let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
        let file = &conversion.files[0];
        let decoded = decode_f32(Path::new(&file.path), 2);
        assert_eq!(decoded[0].len(), 720_000, "10 s + 5 s at 48 kHz");
        // Session 1: 1000 Hz left, 1500 Hz right; session 2 from 10.0 s:
        // 440 Hz left, 660 Hz right.
        for (channel, tones) in decoded.iter().zip([[1000.0, 440.0], [1500.0, 660.0]]) {
            for ((start, end), tone) in [(0, 480_000), (480_000, 720_000)].into_iter().zip(tones) {
                let found = tone_frequency(&channel[start + 2_400..end - 2_400]);
                assert!(
                    (found - tone).abs() < tone * 0.01,
                    "expected {tone} Hz, found {found:.0} Hz"
                );
            }
        }
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(manifest::manifest_path_for(Path::new(&file.path))).unwrap()).unwrap();
        assert!(manifest::tests::schema_errors_for_value(&record).is_empty());
        assert_eq!(record["channel"]["sourceChannelCount"], 2);
        let segments = record["source"]["sgmca"]["segments"].as_array().unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[1]["startText"], "2026-11-22T10:20:00-0500");
        assert_eq!(segments[1]["startSample"], 480_000);
        assert_eq!(fs::read(&sgmca).unwrap(), original, "the source is never modified");
    }

    #[test]
    fn an_sgmca_source_start_catalyst_cannot_parse_is_never_copied() {
        let dir = ConversionTestDir::new();
        let path = dir.0.join("odd_start.sgmca");
        let stream = crate::sgmca::fixtures::synthetic(50, 312, 0, |_| 30);
        fs::write(
            &path,
            crate::sgmca::fixtures::direct_audio(&[
                ("2026-11-22 10:15:30", &stream),
                ("2026-11-22T11:00:00-0500", &stream),
            ]),
        )
        .unwrap();
        let info = crate::sgmca::inspect(&path, None).unwrap();
        assert_eq!(info.segments.len(), 2);
        let now = time::macros::datetime!(2026-11-22 18:00:00 UTC);
        let plan = plan_sgmca_output(Some(&info), None, &manifest::ProbedSource::default(), false, now).unwrap();
        assert_eq!(plan.start_source, "conversion-time");
        assert_eq!(plan.segments.len(), 1);
        assert_eq!(
            plan.segments[0].start_text,
            crate::clock::local_start_text(now).unwrap()
        );
        assert!(
            plan.note
                .contains("The start text of source segment 1 is not in the YYYY-MM-DDTHH:MM:SS±hhmm form"),
            "{}",
            plan.note
        );
        assert!(crate::sgmca::parse_start_text(&plan.segments[0].start_text).is_some());
    }

    /// A copy of one of the owner's sample files (see
    /// scripts/fixtures/sgmca/vendor-notes-samples/README.md), so the
    /// committed file is never an input path.
    fn vendor_sample_copy(dir: &Path, name: &str) -> PathBuf {
        let copy = dir.join(name);
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../scripts/fixtures/sgmca/vendor-notes-samples")
                .join(name),
            &copy,
        )
        .unwrap();
        copy
    }

    fn two_channel_job(src: &Path, out: &Path, mode: &str, format: &str) -> ConvertJob {
        let mut job = court_job(src, out, mode, format);
        job.labels = ["Left", "Right"].map(String::from).to_vec();
        job.chan_vols = vec![1.0; 2];
        job
    }

    #[test]
    fn vendor_notes_samples_convert_to_sgmca_with_every_sample_in_its_session() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        for (name, sessions) in [
            ("tone_1segment.sgmca", vec![480_000u64]),
            ("tone_2sessions.sgmca", vec![480_000, 240_000]),
        ] {
            let sgmca = vendor_sample_copy(&dir.0, name);
            // Mix to Stereo with Normalize Volume: loudnorm, no limiter.
            for mode in ["keep", "split", "stereo"] {
                let out = dir.0.join(format!("{name}-{mode}"));
                fs::create_dir(&out).unwrap();
                let mut job = two_channel_job(&sgmca, &out, mode, "sgmca");
                job.normalize = mode == "stereo";
                let conversion =
                    convert_with_system_ffmpeg(&job, None).unwrap_or_else(|error| panic!("{name} {mode}: {error}"));
                assert_eq!(conversion.files.len(), if mode == "split" { 2 } else { 1 });
                for file in &conversion.files {
                    let info = crate::sgmca::verify_written(Path::new(&file.path), None, None).unwrap();
                    let samples: Vec<u64> = info.segments.iter().map(|segment| segment.samples).collect();
                    assert_eq!(samples, sessions, "{name} {mode}: {}", file.name);
                    assert_eq!(info.samples, sessions.iter().sum::<u64>());
                    let record: serde_json::Value =
                        serde_json::from_slice(&fs::read(manifest::manifest_path_for(Path::new(&file.path))).unwrap())
                            .unwrap();
                    assert!(manifest::tests::schema_errors_for_value(&record).is_empty());
                    assert_eq!(record["source"]["sgmca"]["unreadBytes"], 0);
                    assert_eq!(record["source"]["sgmca"]["omittedSegments"], serde_json::json!([]));
                }
            }
        }
    }

    #[test]
    fn an_sgmca_output_that_does_not_hold_every_source_sample_is_removed() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let sgmca = vendor_sample_copy(&dir.0, "tone_2sessions.sgmca");
        // What the check is given is changed after FFmpeg ran, as if the
        // output had gained a sample, or one session had lost a frame to
        // the next.
        type Change<'a> = &'a dyn Fn(&mut ExpectedSamples);
        let cases: [(&str, Change, &str); 3] = [
            (
                "one-more",
                &|expected| expected.total -= 1,
                "It holds 720000 samples (48 kHz) where the source holds 719999: 1 more, so audio would have been added to the recording.",
            ),
            (
                "one-fewer",
                &|expected| expected.total += 1,
                "It holds 720000 samples (48 kHz) where the source holds 720001: 1 fewer, so audio would have been dropped from the recording.",
            ),
            (
                "moved",
                &|expected| {
                    expected.segments[0] += 960;
                    expected.segments[1] -= 960;
                },
                "Its segment 1 holds 480000 samples (48 kHz) where the matching source session holds 480960, so audio would have moved from one recording session to another.",
            ),
        ];
        for (label, change, message) in cases {
            let out = dir.0.join(label);
            fs::create_dir(&out).unwrap();
            let error =
                convert_with_system_ffmpeg_edited(&two_channel_job(&sgmca, &out, "keep", "sgmca"), None, &|plan| {
                    change(plan.sgmca_output.as_mut().unwrap().expected_samples.as_mut().unwrap())
                })
                .unwrap_err();
            assert_eq!(
                error,
                format!(
                    "tone_2sessions_orig.sgmca failed the SGMCA format check, so the output was removed: {message}"
                ),
                "{label}"
            );
            assert_eq!(
                fs::read_dir(&out).unwrap().count(),
                0,
                "{label}: the output and its record are removed"
            );
        }
    }

    /// A 4-channel PCM WAV of `frames` samples at `rate`, from one filter
    /// graph (so no FFmpeg version drops a frame making it).
    fn synthetic_pcm_wav(path: &Path, rate: u32, frames: u64) {
        synthetic_partial_source(path, rate, frames, &["-c:a", "pcm_s16le", "-f", "wav"]);
    }

    /// `synthetic_pcm_wav`, written with `encoder`.
    fn synthetic_partial_source(path: &Path, rate: u32, frames: u64, encoder: &[&str]) {
        let mut args: Vec<String> = vec![
            "-filter_complex".into(),
            format!(
                "sine=frequency=440:sample_rate={rate},atrim=end_sample={frames},pan=4c|c0=c0|c1=0.5*c0|c2=0.25*c0|c3=0.125*c0[out]"
            ),
            "-map".into(),
            "[out]".into(),
        ];
        args.extend(encoder.iter().map(|arg| arg.to_string()));
        args.extend(["-y".into(), path.to_string_lossy().to_string()]);
        run_ffmpeg(&args);
    }

    /// Lower the granule position of the last page of the bare Ogg stream
    /// at `path` by `by` samples (fixing its CRC): what an encoder or muxer
    /// that trimmed the end of the audio away would write.
    fn lower_last_granule(path: &Path, by: u64) {
        let mut bytes = fs::read(path).unwrap();
        let mut at = 0;
        let mut last = None;
        while at + 27 <= bytes.len() {
            assert_eq!(&bytes[at..at + 4], b"OggS");
            let segments = usize::from(bytes[at + 26]);
            let body: usize = bytes[at + 27..at + 27 + segments]
                .iter()
                .map(|&lace| usize::from(lace))
                .sum();
            let len = 27 + segments + body;
            last = Some((at, len));
            at += len;
        }
        let (at, len) = last.unwrap();
        let granule = u64::from_le_bytes(bytes[at + 6..at + 14].try_into().unwrap()) - by;
        bytes[at + 6..at + 14].copy_from_slice(&granule.to_le_bytes());
        bytes[at + 22..at + 26].fill(0);
        let crc = crate::sgmca::ogg_crc(&bytes[at..at + len]);
        bytes[at + 22..at + 26].copy_from_slice(&crc.to_le_bytes());
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_wav_source_keeps_its_partial_last_opus_frame_in_sgmca_and_a_lost_one_fails() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        // 95,001 samples is 98 whole 20 ms frames and 921 samples: the last
        // Opus frame is mostly real audio and partly padding. 88,201 at
        // 44.1 kHz is 96,001.09 samples at 48 kHz.
        for (rate, frames) in [(48_000, 95_001u64), (44_100, 88_201)] {
            let wav = dir.0.join(format!("synthetic_partial_{rate}.wav"));
            synthetic_pcm_wav(&wav, rate, frames);
            assert_eq!(pcm_wav_frames(&wav), Some((frames, rate)));
            let out = dir.0.join(format!("out_{rate}"));
            fs::create_dir(&out).unwrap();
            let conversion = convert_with_system_ffmpeg(&court_job(&wav, &out, "keep", "sgmca"), None).unwrap();
            let path = Path::new(&conversion.files[0].path);
            let info = crate::sgmca::verify_written(path, None, None).unwrap();
            assert_sgmca_holds_source(path, &info, &wav);
            if rate == 48_000 {
                assert_eq!(info.samples, frames);
            }
        }

        // A FLAC states no exact length in a header DepoAudio reads, so the
        // source is decoded and counted first, and the output must hold
        // that count.
        for (rate, frames) in [(48_000, 95_001u64), (44_100, 88_201)] {
            let flac = dir.0.join(format!("synthetic_partial_{rate}.flac"));
            synthetic_partial_source(&flac, rate, frames, &["-c:a", "flac"]);
            let job = court_job(&flac, &dir.0, "keep", "sgmca");
            assert!(needs_source_sample_count(
                &job,
                &AudioFeed::plain(flac.clone()),
                false,
                false
            ));
            let out = dir.0.join(format!("flac_out_{rate}"));
            fs::create_dir(&out).unwrap();
            let conversion = convert_with_system_ffmpeg(&court_job(&flac, &out, "keep", "sgmca"), None).unwrap();
            let path = Path::new(&conversion.files[0].path);
            let info = crate::sgmca::verify_written(path, None, None).unwrap();
            assert_sgmca_holds_source(path, &info, &flac);
            if rate == 48_000 {
                assert_eq!(info.samples, frames);
            }
        }

        // The same conversion with the end of the audio trimmed away by the
        // last granule, as if the encoder had dropped the partial frame:
        // the conversion fails and nothing is left.
        let wav = dir.0.join("synthetic_partial_48000.wav");
        let out = dir.0.join("lost");
        fs::create_dir(&out).unwrap();
        let error = convert_with_system_ffmpeg_edited(&court_job(&wav, &out, "keep", "sgmca"), None, &|plan| {
            lower_last_granule(&plan.planned[0].path, 921)
        })
        .unwrap_err();
        assert_eq!(
            error,
            "synthetic_partial_48000_orig.sgmca failed the SGMCA format check, so the output was removed: It holds 94080 samples (48 kHz) where the source holds 95001: 921 fewer, so audio would have been dropped from the recording."
        );
        assert_eq!(
            fs::read_dir(&out).unwrap().count(),
            0,
            "the output and its record are removed"
        );
    }

    /// A RIFF (or RF64) WAV header and data for `pcm_wav_frames`.
    fn wav_bytes(tag: u16, block_align: u16, data_len: u32, data_present: usize, rf64_data: Option<u64>) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(if rf64_data.is_some() { b"RF64" } else { b"RIFF" });
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        if let Some(data) = rf64_data {
            bytes.extend_from_slice(b"ds64");
            bytes.extend_from_slice(&28u32.to_le_bytes());
            bytes.extend_from_slice(&0u64.to_le_bytes());
            bytes.extend_from_slice(&data.to_le_bytes());
            bytes.extend_from_slice(&0u64.to_le_bytes());
            bytes.extend_from_slice(&0u32.to_le_bytes());
        }
        // An odd-sized chunk before fmt, with its pad byte.
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&44_100u32.to_le_bytes());
        bytes.extend_from_slice(&(44_100 * u32::from(block_align)).to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data_len.to_le_bytes());
        bytes.resize(bytes.len() + data_present, 0);
        bytes
    }

    #[test]
    fn pcm_wav_frames_are_read_only_when_the_data_length_is_exact() {
        let dir = ConversionTestDir::new();
        let frames = |bytes: Vec<u8>| {
            let path = dir.0.join("header.wav");
            fs::write(&path, bytes).unwrap();
            pcm_wav_frames(&path)
        };
        assert_eq!(frames(wav_bytes(1, 4, 4000, 4000, None)), Some((1000, 44_100)));
        assert_eq!(frames(wav_bytes(3, 4, 4000, 4001, None)), Some((1000, 44_100)), "float");
        assert_eq!(
            frames(wav_bytes(1, 4, u32::MAX, 4000, Some(4000))),
            Some((1000, 44_100)),
            "RF64"
        );
        assert_eq!(frames(wav_bytes(1, 4, 4000, 3999, None)), None, "cut short");
        assert_eq!(frames(wav_bytes(1, 4, 0, 4000, None)), None, "still recording");
        assert_eq!(frames(wav_bytes(1, 4, u32::MAX, 4000, None)), None, "length unknown");
        assert_eq!(frames(wav_bytes(1, 4, 4002, 4002, None)), None, "a partial frame");
        assert_eq!(frames(wav_bytes(0x11, 4, 4000, 4000, None)), None, "IMA ADPCM is coded");
        assert_eq!(frames(b"OggS".to_vec()), None);

        assert_eq!(
            expected_from_source_length(95_001, 48_000),
            ExpectedSamples {
                total: 95_001,
                segments: vec![95_001],
                rounding: 0
            }
        );
        // 88,200 at 44.1 kHz is exactly 96,000 at 48 kHz; 88,201 is 96,001.09.
        assert_eq!(expected_from_source_length(88_200, 44_100).rounding, 0);
        assert_eq!(
            expected_from_source_length(88_201, 44_100),
            ExpectedSamples {
                total: 96_001,
                segments: vec![96_001],
                rounding: 1
            }
        );
    }

    #[test]
    fn the_sample_check_refuses_a_delaying_limiter_and_any_count_that_differs() {
        let dir = ConversionTestDir::new();
        let written = crate::sgmca::inspect(&vendor_sample_copy(&dir.0, "tone_2sessions.sgmca"), None).unwrap();
        let exact = ExpectedSamples {
            total: 720_000,
            segments: vec![480_000, 240_000],
            rounding: 0,
        };
        assert_eq!(check_sgmca_samples(Some(&exact), None, &written), Ok(()));
        assert_eq!(check_sgmca_samples(None, Some("highpass=f=80"), &written), Ok(()));
        // The stereo mix's limiter as this branch builds it: same length,
        // every sample 5 ms late, so the counts alone cannot see it.
        let chain = "pan=stereo|c0=0.5*c0+0.5*c1|c1=0.5*c0+0.5*c1,volume=2,alimiter=limit=0.97:level=false";
        let error = check_sgmca_samples(Some(&exact), Some(chain), &written).unwrap_err();
        assert!(
            error.starts_with("The processing chain includes alimiter without latency compensation"),
            "{error}"
        );
        assert_eq!(delaying_filter(chain), Some("alimiter=limit=0.97:level=false"));
        assert_eq!(
            delaying_filter("[a]alimiter=limit=0.97[b]"),
            Some("[a]alimiter=limit=0.97[b]")
        );
        assert_eq!(delaying_filter("alimiter"), Some("alimiter"));
        assert_eq!(delaying_filter("alimiter=limit=0.97:level=false:latency=1"), None);
        assert_eq!(delaying_filter("alimiter=latency=true:limit=0.97"), None);
        assert_eq!(delaying_filter("loudnorm=I=-16:TP=-1.5:LRA=11,afade=t=in:d=0.5"), None);
        // An FTR recording's channel files joined in front of the chain.
        assert_eq!(
            delaying_filter("[0:a:0][1:a:0]amerge=inputs=2,highpass=f=80[da_out]"),
            None
        );
        assert_eq!(delaying_filter(STEREO_PEAK_LIMITER), None);
        // Leading-silence trim's measured cut and the timeline resets.
        assert_eq!(
            delaying_filter("asetpts=N/SR/TB,atrim=start_sample=9600,asetpts=PTS-STARTPTS,afade=t=in:d=2"),
            None
        );
        // A filter nobody measured is refused, so a delay cannot slip in
        // unseen: the counts would still match.
        for (chain, filter) in [
            ("highpass=f=80,adelay=delays=5", "adelay=delays=5"),
            ("[sp0]pan=mono|c0=c0,apad[op0]", "apad[op0]"),
            ("asetpts=PTS+0.01/TB", "asetpts=PTS+0.01/TB"),
            ("atrim=start=0.5", "atrim=start=0.5"),
            (
                "atrim=start_sample=10:end_sample=20",
                "atrim=start_sample=10:end_sample=20",
            ),
            ("silenceremove=start_periods=1", "silenceremove=start_periods=1"),
        ] {
            assert_eq!(delaying_filter(chain), Some(filter), "{chain}");
        }
        assert!(delay_message("[sp0]adelay=delays=5[op0]").starts_with(
            "The processing chain includes adelay, a filter DepoAudio has not checked for delaying or moving the audio"
        ));
        let joined = ExpectedSamples {
            total: 720_000,
            segments: vec![720_000],
            rounding: 0,
        };
        assert!(check_sgmca_samples(Some(&joined), None, &written)
            .unwrap_err()
            .contains("It has 2 segments where 1 were planned"));
    }

    #[test]
    fn an_sgmca_output_is_never_planned_through_a_delaying_limiter() {
        let dir = ConversionTestDir::new();
        let sgmca = vendor_sample_copy(&dir.0, "tone_2sessions.sgmca");
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let job = two_channel_job(&sgmca, &out, "stereo", "sgmca");
        let probes = ConversionProbes {
            channels: Some(2),
            channel_gains: None,
            proc: Vec::new(),
            probed: manifest::ProbedSource::default(),
            duration_secs: Some(15.0),
            sgmca_source: Some(crate::sgmca::inspect(&sgmca, None).unwrap()),
            fade_out: None,
            leading_trim: None,
            source_samples: None,
        };
        // Every format: the limiter compensates its look-ahead.
        for format in ["sgmca", "wav", "flac", "mp3", "m4a", "opus"] {
            let mut job = job.clone();
            job.format = format.into();
            let mut cleanup = OutputCleanup::default();
            let plan = plan_conversion(&job, &AudioFeed::plain(sgmca.clone()), &probes, &mut cleanup).unwrap();
            let chain = plan.filter_chain.clone().unwrap();
            assert!(chain.ends_with(STEREO_PEAK_LIMITER), "{format}: {chain}");
            assert_eq!(delaying_filter(&chain), None, "{format}: {chain}");
            drop(cleanup);
        }
        // A chain with a filter that delays the audio is refused for every
        // format before FFmpeg runs, not only for SGMCA.
        for format in ["sgmca", "wav", "m4a"] {
            let job = two_channel_job(&sgmca, &out, "keep", format);
            let probes = ConversionProbes {
                channels: Some(2),
                channel_gains: None,
                proc: vec!["alimiter=limit=0.97".into()],
                probed: manifest::ProbedSource::default(),
                duration_secs: Some(15.0),
                sgmca_source: Some(crate::sgmca::inspect(&sgmca, None).unwrap()),
                fade_out: None,
                leading_trim: None,
                source_samples: None,
            };
            let mut cleanup = OutputCleanup::default();
            let error = plan_conversion(&job, &AudioFeed::plain(sgmca.clone()), &probes, &mut cleanup)
                .err()
                .unwrap();
            assert!(
                error.starts_with("The processing chain includes alimiter without latency compensation")
                    && error.ends_with("Nothing was converted."),
                "{format}: {error}"
            );
            drop(cleanup);
        }
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
    }

    #[test]
    fn every_processing_option_builds_a_chain_that_keeps_the_audio_in_place() {
        let mut job = valid_job();
        job.declip = true;
        job.hpf = true;
        job.normalize = true;
        job.fade = true;
        job.trim = true;
        job.mode = "stereo".into();
        let proc = crate::ffmpeg::proc_filters(&job, Some(2.0), Some(60.0), Some(9_600));
        assert!(
            proc.iter().any(|filter| filter == "atrim=start_sample=9600"),
            "{proc:?}"
        );
        let stereo = std::iter::once(stereo_pan_filter(4, &[0.25; 4]))
            .chain(proc.iter().cloned())
            .chain([STEREO_PEAK_LIMITER.to_string()])
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(delaying_filter(&stereo), None, "{stereo}");
        let split = split_filter_complex(4, true, Some(&vec![2.0, 1.0, 0.5, 1.0]), &proc);
        assert_eq!(delaying_filter(&split), None, "{split}");
    }

    #[test]
    fn vendor_notes_two_session_sample_mixes_to_a_stereo_sgmca_with_every_sample_in_place() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let sgmca = vendor_sample_copy(&dir.0, "tone_2sessions.sgmca");
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&two_channel_job(&sgmca, &out, "stereo", "sgmca"), None).unwrap();
        let info = crate::sgmca::verify_written(Path::new(&conversion.files[0].path), None, None).unwrap();
        let samples: Vec<u64> = info.segments.iter().map(|segment| segment.samples).collect();
        assert_eq!(samples, [480_000, 240_000]);
        // Each session's tones start where the session does: nothing late.
        let decoded = decode_f32(Path::new(&conversion.files[0].path), 2);
        let onset = decoded[0].iter().position(|sample| sample.abs() > 0.01).unwrap();
        assert!(onset < 48, "the mix starts {onset} samples late");
    }

    #[test]
    fn a_crashed_recording_records_the_session_and_bytes_left_out() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        // The owner's two-session sample, then a third session whose header
        // was written but whose length never was, and 1000 bytes after it,
        // as a recorder that stopped mid-session leaves the file.
        let sample = vendor_sample_copy(&dir.0, "tone_2sessions.sgmca");
        let mut bytes = fs::read(&sample).unwrap();
        bytes.extend_from_slice(&crate::sgmca::segment_header("2026-11-22T10:30:00-0500", 0).unwrap());
        bytes.extend_from_slice(&vec![0x4f; 1000]);
        let crashed = dir.0.join("crashed.sgmca");
        fs::write(&crashed, &bytes).unwrap();
        let info = crate::sgmca::inspect(&crashed, None).unwrap();
        assert_eq!(info.segments.len(), 2);
        assert_eq!(info.unread_bytes, 1000);
        let queue = manifest::sgmca_audio_streams(&info);
        assert_eq!(queue.unread_bytes, Some(1000));
        assert_eq!(
            serde_json::to_value(&queue).unwrap()["omittedSessions"],
            serde_json::json!([{ "number": 3, "reason": "zero-length-header", "last": true }])
        );

        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&two_channel_job(&crashed, &out, "keep", "sgmca"), None).unwrap();
        let file = &conversion.files[0];
        let written = crate::sgmca::verify_written(Path::new(&file.path), None, None).unwrap();
        assert_eq!(written.samples, 720_000);
        let record: serde_json::Value =
            serde_json::from_slice(&fs::read(manifest::manifest_path_for(Path::new(&file.path))).unwrap()).unwrap();
        assert!(manifest::tests::schema_errors_for_value(&record).is_empty());
        let source = &record["source"]["sgmca"];
        assert_eq!(source["growing"], true);
        assert_eq!(source["unreadBytes"], 1000);
        assert_eq!(
            source["omittedSegments"],
            serde_json::json!([{ "index": 3, "reason": "zero-length-header" }])
        );
        assert!(record["output"]["sgmca"]["startNote"]
            .as_str()
            .unwrap()
            .contains("only its complete audio was converted"));
    }

    /// One 2-channel Opus session of `samples` at 48 kHz with a click (one
    /// sample at 0.9) at each position in `clicks` on its channel.
    fn click_session(dir: &Path, name: &str, samples: u64, clicks: [&[u64]; 2]) -> Vec<u8> {
        let expression = |positions: &[u64]| {
            let terms: Vec<String> = positions.iter().map(|at| format!("eq(n\\,{at})")).collect();
            if terms.is_empty() {
                "0".to_string()
            } else {
                format!("0.9*({})", terms.join("+"))
            }
        };
        let ogg = dir.join(format!("{name}.ogg"));
        run_ffmpeg(&[
            "-f".into(),
            "lavfi".into(),
            "-i".into(),
            format!(
                "aevalsrc=exprs={}|{}:s=48000:d={}",
                expression(clicks[0]),
                expression(clicks[1]),
                samples as f64 / 48_000.0
            ),
            "-c:a".into(),
            "libopus".into(),
            "-mapping_family".into(),
            "255".into(),
            "-frame_duration".into(),
            "20".into(),
            "-b:a".into(),
            "128k".into(),
            "-f".into(),
            "ogg".into(),
            "-y".into(),
            ogg.to_string_lossy().to_string(),
        ]);
        let stream = fs::read(&ogg).unwrap();
        fs::remove_file(&ogg).unwrap();
        stream
    }

    /// Where each click in `samples` starts: the first sample past half the
    /// click's level, one per group (clicks are far apart).
    fn click_positions(samples: &[f32], threshold: f32) -> Vec<i64> {
        let mut found: Vec<i64> = Vec::new();
        for (index, sample) in samples.iter().enumerate() {
            let index = index as i64;
            if sample.abs() > threshold && found.last().is_none_or(|&last| index - last > 4_800) {
                found.push(index);
            }
        }
        found
    }

    fn assert_clicks_at(samples: &[f32], threshold: f32, expected: &[i64], label: &str) {
        let found = click_positions(samples, threshold);
        assert_eq!(
            found.len(),
            expected.len(),
            "{label}: clicks at {found:?}, expected {expected:?}"
        );
        for (&got, &want) in found.iter().zip(expected) {
            // Opus smears a one-sample click over a few samples either side;
            // 1 ms still tells the 6.5 ms pre-skip and 5 ms limiter delays apart.
            assert!(
                (got - want).abs() < 48,
                "{label}: a click at {got} where the source has {want} ({found:?})"
            );
        }
    }

    #[test]
    fn every_output_from_a_two_session_sgmca_keeps_each_click_in_place() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        // Session 1: 3 s, clicks on channel 1 at 0.5 s and 1.5 s and on
        // channel 2 at 1 s. Session 2: 2 s, a click on channel 1 at 0.5 s,
        // which is 3.5 s into the joined audio.
        let first = click_session(&dir.0, "first", 144_000, [&[24_000, 72_000], &[48_000]]);
        let second = click_session(&dir.0, "second", 96_000, [&[24_000], &[]]);
        let sgmca = dir.0.join("clicks.sgmca");
        fs::write(
            &sgmca,
            crate::sgmca::fixtures::direct_audio(&[
                ("2026-11-22T10:15:30-0500", &first),
                ("2026-11-22T10:20:00-0500", &second),
            ]),
        )
        .unwrap();
        let left = [24_000, 72_000, 168_000];
        let right = [48_000];
        let mixed = [24_000, 48_000, 72_000, 168_000];
        for format in ["m4a", "wav", "flac", "mp3", "opus", "sgmca"] {
            for mode in ["keep", "split", "stereo"] {
                let label = format!("{format} {mode}");
                let out = dir.0.join(format!("{format}-{mode}"));
                fs::create_dir(&out).unwrap();
                // Mix to Stereo without Normalize Volume: the peak limiter.
                let job = two_channel_job(&sgmca, &out, mode, format);
                let conversion =
                    convert_with_system_ffmpeg(&job, None).unwrap_or_else(|error| panic!("{label}: {error}"));
                let decoded: Vec<Vec<f32>> = conversion
                    .files
                    .iter()
                    .flat_map(|file| decode_f32(Path::new(&file.path), if mode == "split" { 1 } else { 2 }))
                    .collect();
                match mode {
                    "stereo" => {
                        // Each channel is half of each source channel.
                        assert_clicks_at(&decoded[0], 0.2, &mixed, &label);
                        assert_clicks_at(&decoded[1], 0.2, &mixed, &label);
                    }
                    _ => {
                        assert_clicks_at(&decoded[0], 0.4, &left, &label);
                        assert_clicks_at(&decoded[1], 0.4, &right, &label);
                    }
                }
                for channel in &decoded {
                    // Nothing added before the audio or cut from its end
                    // (AAC pads its last frame, up to 1024 samples).
                    let len = channel.len() as i64;
                    let slack = if format == "m4a" { 1_024 } else { 0 };
                    assert!(
                        (240_000..=240_000 + slack).contains(&len),
                        "{label}: {len} samples where the source holds 240000"
                    );
                }
            }
        }
    }

    // ── Liberty / BIS Digital DCR (synthetic) ───────────────────────────────
    // Built by crate::dcr's test builder from generated tones, following the
    // layout in crate::dcr (owner-supplied interoperability notes; not yet
    // confirmed against a recorded file). No real recording is used.

    use crate::dcr::tests::{waveformatex, DcrBuilder, SECRETS};

    const DCR_RATE: u32 = 16_000;
    const DCR_NAMES: [&str; 4] = ["Judge", "Witness", "Plaintiff", "Defense"];

    fn dcr_tone(index: usize, frame: u64) -> i16 {
        let frequency = [300.0, 450.0, 600.0, 750.0][index];
        crate::dcr::tests::sine_i16(frequency, SYNTHETIC_CHANNEL_VOLUMES[index], DCR_RATE, frame)
    }

    /// A 16-bit PCM DCR with one tone per active channel at
    /// `SYNTHETIC_CHANNEL_VOLUMES`, with a video frame, two attachments and
    /// plain-section markers between atoms.
    fn synthetic_pcm_dcr(dir: &Path, name: &str, mask: u64, legacy: bool) -> PathBuf {
        let channels = mask.count_ones() as usize;
        let mut builder = DcrBuilder::new(crate::dcr::tests::pcm16_format(DCR_RATE), mask, &DCR_NAMES[..channels]);
        if legacy {
            builder = builder.legacy();
        }
        builder.marker(b"audi#mrk");
        let frames = (SYNTHETIC_SECONDS * f64::from(DCR_RATE)) as u64;
        for frame in 0..frames {
            if frame == frames / 3 {
                builder
                    .video_frame(64)
                    .attachment(32, 1)
                    .marker(b"audi#mrk")
                    .attachment(16, 0);
            }
            for channel in 0..channels {
                builder.data.extend_from_slice(&dcr_tone(channel, frame).to_le_bytes());
            }
        }
        builder.write(dir, name)
    }

    fn assert_duration_near(path: &Path, seconds: f64, tolerance: f64) {
        let probe = ffprobe_json(path, &["-show_format"]);
        let duration: f64 = probe["format"]["duration"].as_str().unwrap().parse().unwrap();
        assert!(
            (duration - seconds).abs() < tolerance,
            "{} lasts {duration} s",
            path.display()
        );
    }

    /// The export record of a DCR conversion, checked against the schema and
    /// the files, with no case, docket or password text anywhere in it.
    fn checked_dcr_record(output: &OutputFile, source: &Path, channels: u32) -> serde_json::Value {
        let path = manifest::manifest_path_for(Path::new(&output.path));
        let text = fs::read_to_string(&path).unwrap();
        // Name a leak by its fixture index only, so a failing run never
        // prints the secret text itself.
        for (index, secret) in SECRETS.iter().enumerate() {
            assert!(
                !text.contains(secret),
                "fixture secret #{index} reached the export record"
            );
        }
        let record: serde_json::Value = serde_json::from_str(&text).unwrap();
        let errors = manifest::tests::schema_errors_for_value(&record);
        assert!(errors.is_empty(), "{}: {errors:#?}", path.display());
        assert_eq!(record["source"]["fileName"], basename(&source.to_string_lossy()));
        assert_eq!(record["source"]["sha256"], sha256_of(source));
        assert_eq!(record["output"]["sha256"], sha256_of(Path::new(&output.path)));
        assert_eq!(record["channel"]["sourceChannelCount"], channels);
        let dcr = &record["source"]["dcr"];
        assert_eq!(dcr["channels"], channels);
        // Version 21.00 and later end with checksums; the builder writes
        // correct ones.
        let checksum = if dcr["version"].as_u64().unwrap() >= 2100 {
            crate::dcr::CHECKSUMS_MATCH
        } else {
            crate::dcr::CHECKSUMS_ABSENT
        };
        assert_eq!(dcr["checksum"], checksum);
        assert_eq!(
            dcr["layoutSource"],
            "owner-supplied interoperability notes; not yet confirmed against a recorded file"
        );
        record
    }

    /// The level, in dBFS as a sine peak, of each `encoded_tones` tone
    /// (300 + 150 * index Hz, for `tones` channels) in one decoded channel,
    /// and of what is left once they are all taken out, over the steady
    /// middle second (0.5 s to 1.5 s, past any codec delay). Each tone's
    /// level is a least-squares fit at its frequency, so a lossy codec's
    /// waveform overshoot does not count, but another channel's tone mixed
    /// in shows at that channel's frequency.
    fn tone_levels_db(channel: &[f32], tones: usize) -> (Vec<f64>, f64) {
        const RATE: f64 = 48_000.0;
        let window = &channel[(RATE * 0.5) as usize..(RATE * 1.5) as usize];
        let len = window.len() as f64;
        let mut residual: Vec<f64> = window.iter().map(|&sample| f64::from(sample)).collect();
        let db = |amplitude: f64| 20.0 * amplitude.max(1e-12).log10();
        let levels = (0..tones)
            .map(|index| {
                let omega = 2.0 * std::f64::consts::PI * (300.0 + 150.0 * index as f64) / RATE;
                let (mut sin, mut cos) = (0.0, 0.0);
                for (n, &sample) in window.iter().enumerate() {
                    sin += f64::from(sample) * (omega * n as f64).sin();
                    cos += f64::from(sample) * (omega * n as f64).cos();
                }
                let (a, b) = (2.0 * sin / len, 2.0 * cos / len);
                for (n, value) in residual.iter_mut().enumerate() {
                    *value -= a * (omega * n as f64).sin() + b * (omega * n as f64).cos();
                }
                db(a.hypot(b))
            })
            .collect();
        let rms = (residual.iter().map(|value| value * value).sum::<f64>() / len).sqrt();
        (levels, db(rms * std::f64::consts::SQRT_2))
    }

    /// For a lossy codec (MP3, GSM 6.10): every channel carries its own
    /// tone at the level it was encoded at, within `tolerance_db`, no other
    /// channel's tone is mixed in (each at least 40 dB below), and what is
    /// left besides the tones is at least 20 dB below its own tone. The
    /// tone's fitted level is checked instead of the waveform peak, which a
    /// lossy codec moves by its own coding error.
    fn assert_channel_tones(label: &str, channels: &[Vec<f32>], volumes: &[f64], tolerance_db: f64) {
        assert_eq!(channels.len(), volumes.len(), "{label}: channel count");
        for (index, (channel, volume)) in channels.iter().zip(volumes).enumerate() {
            let (levels, residual) = tone_levels_db(channel, volumes.len());
            let expected = 20.0 * volume.log10();
            eprintln!(
                "{label} channel {index}: tones {levels:.1?} dB, residual {residual:.1} dB, expected {expected:.1} dB"
            );
            assert!(
                (levels[index] - expected).abs() < tolerance_db,
                "{label} channel {index}: own tone {:.1} dB, expected {expected:.1} dB",
                levels[index]
            );
            // Anything else (noise, a misdecoded block) stays well below.
            assert!(
                residual < levels[index] - 20.0,
                "{label} channel {index}: {residual:.1} dB of other signal beside its own tone at {:.1} dB",
                levels[index]
            );
            for (other, level) in levels.iter().enumerate().filter(|(other, _)| *other != index) {
                assert!(
                    *level < levels[index] - 40.0,
                    "{label} channel {index}: channel {other}'s tone is mixed in at {level:.1} dB (own tone {:.1} dB)",
                    levels[index]
                );
            }
        }
    }

    fn assert_split_levels(conversion: &CourtConversion, base: &str, names: &[&str], volumes: &[f64], extension: &str) {
        assert_eq!(conversion.files.len(), names.len());
        let mut previous = f64::INFINITY;
        for ((file, name), volume) in conversion.files.iter().zip(names).zip(volumes) {
            assert_eq!(file.name, format!("{base}_{name}{extension}"));
            let peak = max_volume_db(Path::new(&file.path));
            let expected = 20.0 * volume.log10();
            assert!(
                (peak - expected).abs() < 1.5,
                "{base} {name}: peak {peak} dB, expected {expected:.1} dB"
            );
            assert!(
                peak < previous - 3.0,
                "{name} is not quieter than the channel before it"
            );
            previous = peak;
        }
    }

    #[test]
    fn synthetic_four_channel_pcm_dcr_splits_by_channel_name_in_mask_bit_order() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        // Bits 0, 1, 2 and 4: a gap at bit 3.
        let source = synthetic_pcm_dcr(&dir.0, "hearing.dcr", 0b1_0111, false);
        let original = fs::read(&source).unwrap();
        let out = dir.0.join("split");
        fs::create_dir(&out).unwrap();
        let mut job = court_job(&source, &out, "split", "wav");
        job.labels = ["Mic A", "Mic B", "Mic C", "Mic D"].map(String::from).to_vec();
        let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
        assert_split_levels(&conversion, "hearing", &DCR_NAMES, &SYNTHETIC_CHANNEL_VOLUMES, ".wav");
        for (index, file) in conversion.files.iter().enumerate() {
            assert_audio(Path::new(&file.path), 1, "48000");
            let record = checked_dcr_record(file, &source, 4);
            assert_eq!(record["channel"]["label"], DCR_NAMES[index]);
            assert_eq!(record["channel"]["sourceChannelIndex"], index);
            let dcr = &record["source"]["dcr"];
            assert_eq!(dcr["version"], 2700);
            assert_eq!(dcr["codec"], "PCM 16-bit");
            assert_eq!(dcr["codecTag"], 1);
            assert_eq!(dcr["channelNames"], serde_json::json!(DCR_NAMES));
            assert_eq!(dcr["atoms"], 32_000);
            assert_eq!(dcr["videoFramesSkipped"], 1);
            assert_eq!(dcr["attachmentsSkipped"], 2);
            assert_eq!(dcr["conferenceChanges"], serde_json::json!([]));
        }
        assert_eq!(fs::read(&source).unwrap(), original, "the original is never modified");
        assert!(!conversion.feed.exists(), "the temporary DCR feed was left behind");

        for (mode, format, channels) in [("keep", "flac", 4), ("keep", "wav", 4), ("stereo", "wav", 2)] {
            let out = dir.0.join(format!("{mode}-{format}"));
            fs::create_dir(&out).unwrap();
            let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, mode, format), None).unwrap();
            assert_eq!(conversion.files.len(), 1, "{mode}");
            let output = Path::new(&conversion.files[0].path);
            assert_audio(output, channels, "48000");
            checked_dcr_record(&conversion.files[0], &source, 4);
            if (mode, format) == ("keep", "wav") {
                assert_no_speaker_layout(output);
                assert_keep_levels(output, &SYNTHETIC_CHANNEL_VOLUMES);
            }
        }
    }

    /// A kept DCR's channels are microphones: the WAV names no speaker
    /// positions (FFmpeg did not guess a surround layout such as 4.0).
    fn assert_no_speaker_layout(path: &Path) {
        let probe = ffprobe_json(path, &["-show_streams", "-select_streams", "a:0"]);
        let layout = probe["streams"][0]["channel_layout"].as_str().unwrap_or("");
        assert!(
            layout.is_empty() || layout == "unknown",
            "{} has the speaker layout {layout}",
            path.display()
        );
        let bytes = fs::read(path).unwrap();
        let tag = u16::from_le_bytes([bytes[20], bytes[21]]);
        assert_eq!(&bytes[12..16], b"fmt ");
        if tag == 0xFFFE {
            assert_eq!(&bytes[40..44], &[0; 4], "an extensible header with a speaker mask");
        }
    }

    /// Each channel of a multichannel output at its expected level.
    fn assert_keep_levels(path: &Path, volumes: &[f64]) {
        for (channel, volume) in volumes.iter().enumerate() {
            let peak = max_volume_db_with(path, &format!("pan=mono|c0=c{channel},volumedetect"));
            let expected = 20.0 * volume.log10();
            assert!(
                (peak - expected).abs() < 1.5,
                "{} channel {channel}: {peak} dB, expected {expected:.1} dB",
                path.display()
            );
        }
    }

    #[test]
    fn synthetic_legacy_two_channel_dcr_mixes_and_keeps_channel_order() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let source = synthetic_pcm_dcr(&dir.0, "older.dcr", 0b11, true);
        let out = dir.0.join("keep");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "keep", "wav"), None).unwrap();
        let output = Path::new(&conversion.files[0].path);
        assert_audio(output, 2, "48000");
        let left = max_volume_db_with(output, "pan=mono|c0=c0,volumedetect");
        let right = max_volume_db_with(output, "pan=mono|c0=c1,volumedetect");
        assert!((left - 20.0 * 0.5f64.log10()).abs() < 1.5, "left {left}");
        assert!((right - 20.0 * 0.25f64.log10()).abs() < 1.5, "right {right}");
        let record = checked_dcr_record(&conversion.files[0], &source, 2);
        assert_eq!(record["source"]["dcr"]["version"], 2005);
        // Version 20.xx keeps no channel names.
        assert_eq!(record["channel"]["labels"], serde_json::json!(["Ch 1", "Ch 2"]));
    }

    /// FFmpeg-encoded mono tones for each channel, as (fmt chunk, data).
    fn encoded_tones(dir: &Path, codec: &[&str], volumes: &[f64]) -> Vec<(Vec<u8>, Vec<u8>)> {
        volumes
            .iter()
            .enumerate()
            .map(|(index, volume)| {
                let path = dir.join(format!("tone{index}.wav"));
                let mut args: Vec<String> = vec![
                    "-f".into(),
                    "lavfi".into(),
                    "-i".into(),
                    format!(
                        "sine=frequency={}:duration={SYNTHETIC_SECONDS}:sample_rate=8000",
                        300 + 150 * index
                    ),
                    // lavfi's sine peaks at 1/8 of full scale.
                    "-af".into(),
                    format!("volume={}", volume * 8.0),
                    "-ac".into(),
                    "1".into(),
                ];
                args.extend(codec.iter().map(|arg| arg.to_string()));
                args.extend([
                    "-f".into(),
                    "wav".into(),
                    "-y".into(),
                    path.to_string_lossy().to_string(),
                ]);
                run_ffmpeg(&args);
                let bytes = fs::read(&path).unwrap();
                fs::remove_file(&path).unwrap();
                let mut at = 12;
                let mut fmt = Vec::new();
                loop {
                    let id = &bytes[at..at + 4];
                    let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
                    if id == b"fmt " {
                        fmt = bytes[at + 8..at + 8 + size].to_vec();
                    }
                    if id == b"data" {
                        let end = (at + 8 + size).min(bytes.len());
                        return (fmt, bytes[at + 8..end].to_vec());
                    }
                    at += 8 + size + size % 2;
                }
            })
            .collect()
    }

    /// A DCR from per-channel coded block streams: atom N holds block N of
    /// each channel active then. `joins_at` adds the last channel at that
    /// block (a remote participant, `ptch#str`), silent before it.
    fn coded_dcr(dir: &Path, name: &str, tones: &[(Vec<u8>, Vec<u8>)], joins_at: Option<usize>) -> PathBuf {
        let fmt = tones[0].0.clone();
        let align = usize::from(u16::from_le_bytes([fmt[12], fmt[13]]));
        let blocks = tones.iter().map(|(_, data)| data.len() / align).min().unwrap();
        let channels = tones.len();
        let initial = if joins_at.is_some() { channels - 1 } else { channels };
        let mut builder = DcrBuilder::new(fmt, (1u64 << initial) - 1, &DCR_NAMES[..initial]);
        builder.marker(b"audi#mrk");
        for block in 0..blocks {
            if joins_at == Some(block) {
                // The participant brings the next channel bit.
                builder.participant(true, &(1u64 << initial).to_string());
            }
            let present = if joins_at.is_some_and(|at| block < at) {
                initial
            } else {
                channels
            };
            for (_, data) in &tones[..present] {
                builder
                    .data
                    .extend_from_slice(&data[block * align..(block + 1) * align]);
            }
        }
        builder.write(dir, name)
    }

    #[test]
    fn synthetic_a_law_dcr_splits_in_channel_order() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let volumes = &SYNTHETIC_CHANNEL_VOLUMES[..2];
        let tones = encoded_tones(&dir.0, &["-c:a", "pcm_alaw"], volumes);
        assert_eq!(&tones[0].0[..2], &[6, 0], "A-law format tag");
        let source = coded_dcr(&dir.0, "alaw.dcr", &tones, None);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "split", "wav"), None).unwrap();
        assert_split_levels(&conversion, "alaw", &DCR_NAMES[..2], volumes, ".wav");
        let record = checked_dcr_record(&conversion.files[0], &source, 2);
        assert_eq!(record["source"]["dcr"]["codec"], "A-law");
    }

    #[test]
    fn synthetic_ima_adpcm_dcr_merges_channels_and_silences_a_late_participant() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let volumes = &SYNTHETIC_CHANNEL_VOLUMES[..3];
        let tones = encoded_tones(&dir.0, &["-c:a", "adpcm_ima_wav"], volumes);
        assert_eq!(&tones[0].0[..2], &[0x11, 0], "IMA ADPCM format tag");
        let align = usize::from(u16::from_le_bytes([tones[0].0[12], tones[0].0[13]]));
        let blocks = tones[0].1.len() / align;
        let joins_at = blocks / 2;
        let source = coded_dcr(&dir.0, "remote.dcr", &tones, Some(joins_at));
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "split", "wav"), None).unwrap();
        assert!(!conversion.feed.exists(), "the merged feed was left behind");
        let names = ["Judge", "Witness", "Ch 3"];
        assert_eq!(conversion.files.len(), 3);
        for (file, name) in conversion.files.iter().zip(names) {
            assert_eq!(file.name, format!("remote_{}.wav", name.replace(' ', "_")));
            assert_duration_near(Path::new(&file.path), SYNTHETIC_SECONDS, 0.3);
        }
        // Each channel at its own level, in order.
        for (index, file) in conversion.files.iter().enumerate().take(2) {
            let peak = max_volume_db(Path::new(&file.path));
            let expected = 20.0 * volumes[index].log10();
            assert!((peak - expected).abs() < 1.5, "{}: {peak} vs {expected}", file.name);
        }
        // The participant's channel is silent before the join and at its
        // own level after it; the timeline stays aligned.
        let late = Path::new(&conversion.files[2].path);
        let samples_per_block = u16::from_le_bytes([tones[0].0[18], tones[0].0[19]]);
        let join_seconds = (joins_at as f64 * f64::from(samples_per_block)) / 8000.0;
        let before = max_volume_db_between(late, 0.0, join_seconds - 0.05);
        let after = max_volume_db_between(late, join_seconds + 0.05, SYNTHETIC_SECONDS - 0.1);
        assert!(before < -80.0, "the absent channel is not silent: {before} dB");
        assert!(
            (after - 20.0 * volumes[2].log10()).abs() < 1.5,
            "after joining: {after} dB"
        );

        let record = checked_dcr_record(&conversion.files[2], &source, 3);
        let dcr = &record["source"]["dcr"];
        assert_eq!(dcr["codec"], "IMA ADPCM");
        assert_eq!(dcr["conferenceChangeCount"], 1);
        let change = &dcr["conferenceChanges"][0];
        assert_eq!(change["kind"], "joined");
        assert_eq!(change["channelsAfter"], 3);
        assert_eq!(change["channelMask"], 4);
        assert_eq!(change["atPerChannelByte"], (joins_at * align) as u64);

        // Keep Original and Mix to Stereo read the same merged feed.
        for (mode, channels) in [("keep", 3), ("stereo", 2)] {
            let out = dir.0.join(mode);
            fs::create_dir(&out).unwrap();
            let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, mode, "wav"), None).unwrap();
            assert_eq!(conversion.files.len(), 1, "{mode}");
            let output = Path::new(&conversion.files[0].path);
            assert_audio(output, channels, "48000");
            checked_dcr_record(&conversion.files[0], &source, 3);
            if mode == "keep" {
                assert_no_speaker_layout(output);
                assert_keep_levels(output, &volumes[..2]);
                let late = max_volume_db_between(output, 0.0, join_seconds - 0.05);
                assert!(
                    late > -80.0,
                    "the mix of the first channels is not silent before the join"
                );
            }
            assert!(!conversion.feed.exists(), "{mode}: the merged feed was left behind");
        }
    }

    #[test]
    fn synthetic_mp3_and_gsm_dcrs_merge_their_mono_channels() {
        // Coding error moves a lossy codec's level a little; channel bleed
        // is checked separately and strictly (assert_channel_tones).
        const LOSSY_TONE_TOLERANCE_DB: f64 = 1.5;
        if !court_fixture_tools_available() {
            return;
        }
        let encoders = String::from_utf8_lossy(
            &std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-encoders"])
                .output()
                .unwrap()
                .stdout,
        )
        .to_string();
        let dir = ConversionTestDir::new();
        let volumes = &SYNTHETIC_CHANNEL_VOLUMES[..2];
        for (encoder, codec, tag) in [("libmp3lame", "MP3", 0x55u8), ("libgsm_ms", "GSM 6.10", 0x31)] {
            if !encoders.contains(encoder) {
                // FFmpeg has no built-in GSM or MP3 encoder, so these
                // fixtures need the external ones; CI must have them.
                assert!(
                    std::env::var_os("DEPOAUDIO_REQUIRE_FFMPEG_TESTS").is_none_or(|value| value != "1"),
                    "DEPOAUDIO_REQUIRE_FFMPEG_TESTS=1, but FFmpeg has no {encoder} encoder for the {codec} fixture"
                );
                eprintln!("skipping {codec}: FFmpeg has no {encoder} encoder");
                continue;
            }
            let mut args = vec!["-c:a", encoder];
            if encoder == "libmp3lame" {
                args.extend(["-b:a", "64k"]);
            }
            let tones = encoded_tones(&dir.0, &args, volumes);
            assert_eq!(tones[0].0[0], tag, "{codec} format tag");
            let name = format!("{}.dcr", encoder);
            let source = coded_dcr(&dir.0, &name, &tones, None);
            let out = dir.0.join(encoder);
            fs::create_dir(&out).unwrap();
            let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "split", "wav"), None)
                .unwrap_or_else(|error| panic!("{codec}: {error}"));
            assert_eq!(conversion.files.len(), 2, "{codec}");
            let split: Vec<Vec<f32>> = conversion
                .files
                .iter()
                .zip(&DCR_NAMES[..2])
                .map(|(file, name)| {
                    assert_eq!(file.name, format!("{encoder}_{name}.wav"));
                    decode_f32_default(Path::new(&file.path), 1).remove(0)
                })
                .collect();
            assert_channel_tones(&format!("{codec} split"), &split, volumes, LOSSY_TONE_TOLERANCE_DB);
            assert!(!conversion.feed.exists(), "{codec}: the merged feed was left behind");
            let record = checked_dcr_record(&conversion.files[0], &source, 2);
            assert_eq!(record["source"]["dcr"]["codec"], codec);
            for (mode, channels) in [("keep", 2), ("stereo", 2)] {
                let out = dir.0.join(format!("{encoder}-{mode}"));
                fs::create_dir(&out).unwrap();
                let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, mode, "wav"), None)
                    .unwrap_or_else(|error| panic!("{codec} {mode}: {error}"));
                let output = Path::new(&conversion.files[0].path);
                let probe = ffprobe_json(output, &["-show_streams", "-select_streams", "a:0"]);
                assert_eq!(probe["streams"][0]["channels"], channels, "{codec} {mode}");
                // MP3 decoding adds the encoder's padding.
                assert_duration_near(output, SYNTHETIC_SECONDS, 0.3);
                if mode == "keep" {
                    assert_channel_tones(
                        &format!("{codec} keep"),
                        &decode_f32_default(output, 2),
                        volumes,
                        LOSSY_TONE_TOLERANCE_DB,
                    );
                }
                checked_dcr_record(&conversion.files[0], &source, 2);
            }
        }
    }

    #[test]
    fn synthetic_dcr_with_a_short_last_atom_records_the_bytes_left_out() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let mut builder = DcrBuilder::new(crate::dcr::tests::pcm16_format(DCR_RATE), 0b11, &DCR_NAMES[..2]);
        builder.marker(b"audi#mrk");
        for frame in 0..(SYNTHETIC_SECONDS * f64::from(DCR_RATE)) as u64 {
            builder.atom(&[&dcr_tone(0, frame).to_le_bytes(), &dcr_tone(1, frame).to_le_bytes()]);
        }
        builder.data.extend_from_slice(&[7, 7]);
        let source = builder.write(&dir.0, "short.dcr");
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "split", "wav"), None).unwrap();
        assert_split_levels(
            &conversion,
            "short",
            &DCR_NAMES[..2],
            &SYNTHETIC_CHANNEL_VOLUMES[..2],
            ".wav",
        );
        let record = checked_dcr_record(&conversion.files[0], &source, 2);
        assert_eq!(record["source"]["dcr"]["trailingBytesDropped"], 2);
    }

    /// Mono tones in a format FFmpeg writes as one interleavable block per
    /// sample, as (one-channel WAVEFORMATEX, data).
    fn pcm_family_tones(dir: &Path, codec: &str, volumes: &[f64]) -> Vec<(Vec<u8>, Vec<u8>)> {
        let tones = encoded_tones(dir, &["-c:a", codec], volumes);
        // FFmpeg may write WAVE_FORMAT_EXTENSIBLE for 24/32-bit; the DCR
        // keeps a plain one-channel WAVEFORMATEX.
        tones
            .into_iter()
            .map(|(fmt, data)| {
                let mut plain = fmt[..16].to_vec();
                if u16::from_le_bytes([plain[0], plain[1]]) == 0xFFFE {
                    plain[..2].copy_from_slice(&fmt[24..26]);
                }
                plain.extend_from_slice(&0u16.to_le_bytes());
                (plain, data)
            })
            .collect()
    }

    #[test]
    fn synthetic_mu_law_and_8_24_32_bit_pcm_dcrs_split_in_channel_order() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let volumes = &SYNTHETIC_CHANNEL_VOLUMES[..2];
        for (codec, tag, name) in [
            ("pcm_mulaw", 7u16, "mu-law"),
            ("pcm_u8", 1, "PCM 8-bit"),
            ("pcm_s24le", 1, "PCM 24-bit"),
            ("pcm_s32le", 1, "PCM 32-bit"),
        ] {
            let tones = pcm_family_tones(&dir.0, codec, volumes);
            assert_eq!(u16::from_le_bytes([tones[0].0[0], tones[0].0[1]]), tag, "{codec}");
            let file = format!("{codec}.dcr");
            let source = coded_dcr(&dir.0, &file, &tones, None);
            let out = dir.0.join(codec);
            fs::create_dir(&out).unwrap();
            let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "split", "wav"), None)
                .unwrap_or_else(|error| panic!("{codec}: {error}"));
            assert_split_levels(&conversion, codec, &DCR_NAMES[..2], volumes, ".wav");
            let record = checked_dcr_record(&conversion.files[0], &source, 2);
            assert_eq!(record["source"]["dcr"]["codec"], name);
        }

        // 8-bit PCM silence is 0x80: a late participant's channel is silent
        // before the join, not a full-scale offset.
        let tones = pcm_family_tones(&dir.0, "pcm_u8", &SYNTHETIC_CHANNEL_VOLUMES[..2]);
        let joins_at = tones[0].1.len() / 2;
        let source = coded_dcr(&dir.0, "u8-remote.dcr", &tones, Some(joins_at));
        let out = dir.0.join("u8-remote");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "split", "wav"), None).unwrap();
        let late = Path::new(&conversion.files[1].path);
        let join_seconds = joins_at as f64 / 8000.0;
        assert!(max_volume_db_between(late, 0.0, join_seconds - 0.05) < -40.0);
    }

    /// Scan and the waveform read a DCR through the same prepared feed as
    /// conversion; here their FFmpeg passes run on a merged (IMA ADPCM) feed
    /// with FFmpeg from PATH (the passes themselves need the Tauri sidecar).
    #[test]
    fn scan_and_waveform_passes_read_a_merged_dcr_feed() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let volumes = &SYNTHETIC_CHANNEL_VOLUMES[..3];
        let tones = encoded_tones(&dir.0, &["-c:a", "adpcm_ima_wav"], volumes);
        let source = coded_dcr(&dir.0, "scan.dcr", &tones, None);
        let prepared = prepare_audio_feed_with_cancel(&source, FeedOptions::default(), None).unwrap();
        let prepared = finish_with_system_ffmpeg(prepared).unwrap();
        let feed = prepared.path().to_string_lossy().to_string();
        for (channel, volume) in volumes.iter().enumerate() {
            let mut args = crate::helpers::safe_ffmpeg_input_prelude();
            args.extend(crate::analysis::loudness_pass_args(&feed, Some(channel as u32), "180"));
            let output = std::process::Command::new("ffmpeg").args(&args).output().unwrap();
            assert!(output.status.success(), "channel {channel}");
            let log = String::from_utf8_lossy(&output.stderr);
            let peak: f64 = log
                .lines()
                .rev()
                .find_map(|line| line.trim().strip_prefix("Peak:"))
                .and_then(|rest| rest.trim().trim_end_matches("dBFS").trim().parse().ok())
                .unwrap_or_else(|| panic!("no peak for channel {channel}: {log}"));
            let expected = 20.0 * volume.log10();
            assert!(
                (peak - expected).abs() < 1.5,
                "channel {channel}: {peak} dB vs {expected:.1}"
            );
        }
        let output = std::process::Command::new("ffmpeg")
            .args(crate::waveform::decode_args(&prepared))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let peaks = crate::waveform::samples_to_peaks(&output.stdout, 400).unwrap();
        assert!(!peaks.is_empty());
        let feed_path = prepared.path().to_path_buf();
        drop(prepared);
        assert!(!feed_path.exists(), "the merged feed was left behind");
    }

    #[test]
    fn unsupported_dcrs_fail_before_any_output_and_sealed_audio_is_silent() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let mut speex = DcrBuilder::new(waveformatex(0xA109, 8000, 1000, 20, 16, &[0; 4]), 0b1, &[]);
        speex.atom(&[&[0; 20]]);
        let source = speex.write(&dir.0, "speex.dcr");
        let out = dir.0.join("out-speex");
        fs::create_dir(&out).unwrap();
        let error = convert_with_system_ffmpeg(&court_job(&source, &out, "split", "wav"), None)
            .err()
            .unwrap();
        assert_eq!(error, crate::dcr::SPEEX_MESSAGE);
        assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "something was written");

        // A tone whose middle third is a password-protected section.
        let mut sealed = DcrBuilder::new(crate::dcr::tests::pcm16_format(DCR_RATE), 0b1, &["Judge"]);
        sealed.have_rec = "1";
        let frames = (SYNTHETIC_SECONDS * f64::from(DCR_RATE)) as u64;
        sealed.marker(b"audi#mrk");
        for frame in 0..frames {
            if frame == frames / 3 {
                sealed.marker(b"audi#crp");
            }
            if frame == 2 * frames / 3 {
                sealed.marker(b"audi#mrk");
            }
            sealed.atom(&[&dcr_tone(0, frame).to_le_bytes()]);
        }
        let source = sealed.write(&dir.0, "sealed.dcr");
        let out = dir.0.join("out-sealed");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "keep", "wav"), None).unwrap();
        let output = Path::new(&conversion.files[0].path);
        assert_duration_near(output, SYNTHETIC_SECONDS, 0.01);
        let third = SYNTHETIC_SECONDS / 3.0;
        assert!(max_volume_db_between(output, third + 0.01, 2.0 * third - 0.01) < -80.0);
        assert!(max_volume_db_between(output, 0.0, third - 0.01) > -20.0);
        let record = checked_dcr_record(&conversion.files[0], &source, 1);
        assert_eq!(record["source"]["dcr"]["sealedSections"], 1);
        assert_eq!(record["source"]["dcr"]["passwordSet"], true);
    }

    /// The 16-bit samples of a PCM WAV output, interleaved.
    fn wav_i16(path: &Path) -> Vec<i16> {
        let bytes = fs::read(path).unwrap();
        let mut at = 12;
        while at + 8 <= bytes.len() {
            let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            if &bytes[at..at + 4] == b"data" {
                let end = (at + 8 + size).min(bytes.len());
                return bytes[at + 8..end]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| i16::from_le_bytes([pair[0], pair[1]]))
                    .collect();
            }
            at += 8 + size + size % 2;
        }
        panic!("{} has no data chunk", path.display());
    }

    // ── The owner's two synthetic DCR samples (rebuild-tool tones) ─────────

    #[test]
    fn the_owners_plain_dcr_sample_converts_to_twelve_seconds_of_judge_and_witness() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let source = crate::dcr::tests::fixture(crate::dcr::tests::PLAIN_SAMPLE);
        let original = fs::read(&source).unwrap();
        let out = dir.0.join("split");
        fs::create_dir(&out).unwrap();
        let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "split", "wav"), None).unwrap();
        let names: Vec<&str> = conversion.files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(
            names,
            ["test_tone_2ch_8k_pcm16_Judge.wav", "test_tone_2ch_8k_pcm16_Witness.wav"]
        );
        for file in &conversion.files {
            let output = Path::new(&file.path);
            let samples = wav_i16(output).len() as i64;
            assert!((samples - 12 * 48_000).abs() <= 48, "{}: {samples} samples", file.name);
            assert_duration_near(output, 12.0, 0.002);
            let path = manifest::manifest_path_for(output);
            let record: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            assert!(manifest::tests::schema_errors_for_value(&record).is_empty());
            let dcr = &record["source"]["dcr"];
            assert_eq!(dcr["checksum"], "checksums match");
            assert_eq!(dcr["version"], 2700);
            assert_eq!(dcr["timeZone"]["display"], "GMT+0:00");
            assert!(dcr.get("redactions").is_none());
        }
        assert_eq!(fs::read(&source).unwrap(), original, "the sample is never modified");
    }

    #[test]
    fn the_owners_redacted_dcr_sample_is_silent_from_four_to_six_seconds_on_both_channels() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let convert = |name: &str| {
            let source = crate::dcr::tests::fixture(name);
            let out = dir.0.join(name);
            fs::create_dir(&out).unwrap();
            let conversion = convert_with_system_ffmpeg(&court_job(&source, &out, "keep", "wav"), None).unwrap();
            assert_eq!(conversion.files.len(), 1);
            conversion.files[0].clone()
        };
        let plain = convert(crate::dcr::tests::PLAIN_SAMPLE);
        let redacted = convert(crate::dcr::tests::REDACTED_SAMPLE);
        let plain_samples = wav_i16(Path::new(&plain.path));
        let redacted_samples = wav_i16(Path::new(&redacted.path));
        assert_eq!(
            plain_samples.len(),
            redacted_samples.len(),
            "a blacked-out segment keeps its length"
        );
        // 48 kHz stereo; 10 ms either side of each edge is left to the
        // resampler's filter.
        let frame = |seconds: f64| (seconds * 48_000.0) as usize;
        for (index, (a, b)) in plain_samples
            .as_chunks::<2>()
            .0
            .iter()
            .zip(redacted_samples.as_chunks::<2>().0.iter())
            .enumerate()
        {
            if (frame(4.01)..frame(5.99)).contains(&index) {
                assert_eq!(*b, [0, 0], "frame {index} is not silent");
            } else if index < frame(3.99) || index >= frame(6.01) {
                assert_eq!(a, b, "frame {index} differs from the plain sample");
            }
        }
        assert!(
            plain_samples[frame(4.01) * 2..frame(5.99) * 2]
                .iter()
                .any(|sample| *sample != 0),
            "the plain sample has audio there"
        );
        let record: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(manifest::manifest_path_for(Path::new(&redacted.path))).unwrap())
                .unwrap();
        assert!(manifest::tests::schema_errors_for_value(&record).is_empty());
        assert_eq!(
            record["source"]["dcr"]["redactions"],
            serde_json::json!([{ "startSecs": 4.0, "endSecs": 6.0, "action": "black", "applied": true }])
        );
        let text = record.to_string();
        assert!(
            !text.contains("Begin Redaction") && !text.contains("play_black"),
            "{text}"
        );
    }

    // ── Sample alignment guard ───────────────────────────────────────────────
    //
    // Legal audio must line up with its source sample for sample. Every mode,
    // processing option, format and sample rate the Convert tab offers is run
    // through the conversion plan on a synthetic click-and-tone fixture, and
    // each output channel must (a) have the source's length at the output
    // rate, shortened only by leading-silence trim, and only by silence, and
    // (b) have its clicks at the source's positions (cross-correlation lag 0).
    // A filter with latency, such as alimiter without `latency=1` (5 ms late,
    // last 5 ms cut), fails (b); padding or a cut fails (a).
    mod alignment {
        use super::*;
        use std::sync::atomic::AtomicUsize;
        use std::sync::Mutex;

        const RATE: usize = 48_000;
        const LEN: usize = 3 * RATE;
        const CHANNELS: usize = 4;
        /// Exact digital silence before the tone, for Trim silence to remove.
        const LEAD_SILENCE: usize = RATE / 2;
        /// Clicks on every channel: two mid-recording, one 300 samples before
        /// the end, inside the span a 5 ms delay would cut.
        const CLICKS: [usize; 3] = [60_017, 100_005, LEN - 300];
        /// BWF start written into the fixture, so the record's start can be
        /// checked: 10 s after midnight.
        const TIME_REFERENCE: u64 = 10 * RATE as u64;

        fn fixture_sample(n: usize, channel: usize) -> f64 {
            if CLICKS.contains(&n) {
                0.9
            } else if n < LEAD_SILENCE {
                0.0
            } else {
                // The tone drops 20 dB within 50 ms of each click, so after
                // loudnorm, lossy coding and resampling the click is still
                // by far the strongest thing near it; it stays above the
                // -50 dBFS trim threshold.
                let near_click = CLICKS.iter().any(|&click| n.abs_diff(click) <= RATE / 20);
                let level = if near_click { 0.005 } else { 0.05 };
                let frequency = 300.0 + 110.0 * channel as f64;
                level * (2.0 * std::f64::consts::PI * frequency * (n - LEAD_SILENCE) as f64 / RATE as f64).sin()
            }
        }

        /// The sample the output must start at after Trim silence, found
        /// here without FFmpeg: the first sample of the 16-bit fixture above
        /// -50 dBFS on any of `channels`.
        fn first_sample_above_threshold(channels: &[usize]) -> usize {
            let threshold = 10f64.powf(-50.0 / 20.0);
            (0..LEN)
                .find(|&n| {
                    channels.iter().any(|&channel| {
                        let quantized = (fixture_sample(n, channel) * 32767.0).round() / 32768.0;
                        quantized.abs() > threshold
                    })
                })
                .unwrap()
        }

        /// The fixture: 3 s of 4-channel 48 kHz 16-bit WAV with a bext start,
        /// in hound's default 3.1 layout, as a court WAV may well be.
        fn write_fixture(dir: &Path) -> PathBuf {
            let raw = dir.join("alignment_raw.wav");
            let spec = hound::WavSpec {
                channels: CHANNELS as u16,
                sample_rate: RATE as u32,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut writer = hound::WavWriter::create(&raw, spec).unwrap();
            for n in 0..LEN {
                for channel in 0..CHANNELS {
                    writer
                        .write_sample((fixture_sample(n, channel) * 32767.0).round() as i16)
                        .unwrap();
                }
            }
            writer.finalize().unwrap();
            let fixture = dir.join("alignment_fixture.wav");
            // The layout is kept as hound wrote it (3.1): Keep Original must
            // refuse the formats that would code the fourth channel as LFE.
            run_ffmpeg(&[
                "-i".into(),
                raw.to_string_lossy().to_string(),
                "-c:a".into(),
                "pcm_s16le".into(),
                "-write_bext".into(),
                "1".into(),
                "-metadata".into(),
                format!("time_reference={TIME_REFERENCE}"),
                "-y".into(),
                fixture.to_string_lossy().to_string(),
            ]);
            fs::remove_file(&raw).unwrap();
            fixture
        }

        /// Every channel of `path` as 32-bit float at `rate`. Opus outputs
        /// always decode at 48 kHz.
        fn decode(path: &Path, rate: usize, channels: usize, opus: bool) -> Vec<Vec<f32>> {
            let mut command = std::process::Command::new("ffmpeg");
            command.args(["-nostdin", "-hide_banner", "-loglevel", "error"]);
            // An SGMCA (Direct Audio) file is read the way the app reads it:
            // through its concat script over the recording sessions.
            let (input, _feed) = if path.extension().is_some_and(|extension| extension == "sgmca") {
                let (input, feed) = ffmpeg_input(path);
                (input, Some(feed))
            } else {
                let mut input = Vec::new();
                if opus {
                    input.extend(["-c:a".to_string(), "libopus".to_string()]);
                }
                input.extend(["-i".to_string(), path.to_string_lossy().to_string()]);
                (input, None)
            };
            let output = command
                .args(&input)
                .args(["-map", "0:a:0", "-f", "f32le", "-ar", &rate.to_string(), "-"])
                .output()
                .unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            let samples: Vec<f32> = output
                .stdout
                .as_chunks::<4>()
                .0
                .iter()
                .map(|bytes| f32::from_le_bytes(*bytes))
                .collect();
            (0..channels)
                .map(|channel| samples.iter().skip(channel).step_by(channels).copied().collect())
                .collect()
        }

        /// The stream's own length in samples, as the container states it.
        fn stated_samples(path: &Path) -> Option<i64> {
            let probe = ffprobe_json(path, &["-show_streams", "-select_streams", "a:0"]);
            probe["streams"][0]["duration_ts"].as_i64()
        }

        /// Cross-correlation lag (in samples) of `output` against
        /// `reference`, searched within `max_lag` of `offset`, over the
        /// neighbourhood of each click. 0 means the clicks line up.
        fn click_lag(reference: &[f32], output: &[f32], centers: &[usize], offset: i64, max_lag: i64) -> i64 {
            const HALF_WINDOW: i64 = 64;
            let mut best = (0_i64, f64::MIN);
            for lag in -max_lag..=max_lag {
                let mut sum = 0.0_f64;
                for &center in centers {
                    for n in center as i64 - HALF_WINDOW..=center as i64 + HALF_WINDOW {
                        let m = n + offset + lag;
                        if n < 0 || m < 0 || n as usize >= reference.len() || m as usize >= output.len() {
                            continue;
                        }
                        sum += f64::from(reference[n as usize]) * f64::from(output[m as usize]);
                    }
                }
                if sum > best.1 {
                    best = (lag, sum);
                }
            }
            best.0
        }

        #[derive(Clone, Copy, Debug)]
        struct Options {
            normalize: bool,
            trim: bool,
            fade: bool,
            hpf: bool,
            declip: bool,
            auto_level: bool,
        }

        impl Options {
            fn from_bits(bits: u32) -> Self {
                Self {
                    normalize: bits & 1 != 0,
                    trim: bits & 2 != 0,
                    fade: bits & 4 != 0,
                    hpf: bits & 8 != 0,
                    declip: bits & 16 != 0,
                    auto_level: bits & 32 != 0,
                }
            }
        }

        #[derive(Clone, Debug)]
        struct Case {
            mode: &'static str,
            format: &'static str,
            rate: usize,
            mp3_bitrate: u32,
            options: Options,
        }

        /// Every mode × every processing combination at WAV 48 kHz, then every
        /// format × offered rate (Opus and SGMCA are fixed at 48 kHz) ×
        /// mode with all processing off and all on, and each MP3 bitrate.
        fn cases() -> Vec<Case> {
            let modes = ["keep", "stereo", "split"];
            let mut cases = Vec::new();
            for mode in modes {
                for bits in 0..64 {
                    let options = Options::from_bits(bits);
                    // Keep Original rejects auto-level (validate_convert_request).
                    if mode == "keep" && options.auto_level {
                        continue;
                    }
                    cases.push(Case {
                        mode,
                        format: "wav",
                        rate: 48_000,
                        mp3_bitrate: 192,
                        options,
                    });
                }
            }
            let formats: [(&str, &[usize]); 6] = [
                ("wav", &[22_050, 44_100]),
                ("flac", &[22_050, 44_100, 48_000]),
                ("mp3", &[22_050, 44_100, 48_000]),
                ("m4a", &[22_050, 44_100, 48_000]),
                ("opus", &[48_000]),
                ("sgmca", &[48_000]),
            ];
            for (format, rates) in formats {
                for &rate in rates {
                    for mode in modes {
                        for all in [false, true] {
                            let mut options = Options::from_bits(if all { 63 } else { 0 });
                            options.auto_level &= mode != "keep";
                            cases.push(Case {
                                mode,
                                format,
                                rate,
                                mp3_bitrate: 192,
                                options,
                            });
                        }
                    }
                }
            }
            for mp3_bitrate in [128, 320] {
                for mode in modes {
                    cases.push(Case {
                        mode,
                        format: "mp3",
                        rate: 44_100,
                        mp3_bitrate,
                        options: Options::from_bits(0),
                    });
                }
            }
            cases
        }

        /// Converts `case` and returns every alignment problem found.
        fn check_case(case: &Case, fixture: &Path, root: &Path, index: usize) -> Vec<String> {
            let out = root.join(format!("case_{index}"));
            fs::create_dir(&out).unwrap();
            let mut job = court_job(fixture, &out, case.mode, case.format);
            job.rate = case.rate.to_string();
            job.mp3_bitrate = case.mp3_bitrate;
            job.normalize = case.options.normalize;
            job.trim = case.options.trim;
            job.fade = case.options.fade;
            job.hpf = case.options.hpf;
            job.declip = case.options.declip;
            job.auto_level = case.options.auto_level;
            let gains = case.options.auto_level.then(|| vec![1.0, 2.0, 0.5, 1.25]);
            let result = convert_with_system_ffmpeg(&job, gains);
            // MP3 folds four channels into two, and AAC and Opus code the
            // fourth channel of this 3.1 source as a low-pass LFE, so Keep
            // Original refuses them before writing anything.
            if case.mode == "keep" && matches!(case.format, "mp3" | "m4a" | "opus") {
                let written = fs::read_dir(&out).unwrap().count();
                return match result {
                    Err(error) if error.contains("4-channel") && written == 0 => Vec::new(),
                    Err(error) => vec![format!("{case:?}: refused with {error:?}, {written} files left")],
                    Ok(_) => vec![format!(
                        "{case:?}: 4-channel {} now converts; check it here",
                        case.format
                    )],
                };
            }
            let conversion = match result {
                Ok(conversion) => conversion,
                Err(error) => return vec![format!("{case:?}: conversion failed: {error}")],
            };

            let opus = matches!(case.format, "opus" | "sgmca");
            let decode_rate = if opus { RATE } else { case.rate };
            let scale = |n: usize| ((n as u64 * decode_rate as u64 + RATE as u64 / 2) / RATE as u64) as usize;
            let full_len = scale(LEN);
            let reference = decode(fixture, decode_rate, CHANNELS, false).swap_remove(0);
            let centers: Vec<usize> = CLICKS.iter().map(|&n| scale(n)).collect();
            let trimmed = case.options.trim && case.mode != "split";
            let expected_channels = match case.mode {
                "keep" => CHANNELS,
                "stereo" => 2,
                _ => 1,
            };
            let expected_outputs = if case.mode == "split" { CHANNELS } else { 1 };
            let mut problems = Vec::new();
            if conversion.files.len() != expected_outputs {
                problems.push(format!("{case:?}: {} outputs", conversion.files.len()));
            }

            for file in &conversion.files {
                let path = Path::new(&file.path);
                let probe = ffprobe_json(path, &["-show_streams", "-select_streams", "a:0"]);
                if probe["streams"][0]["channels"] != expected_channels as u64 {
                    problems.push(format!(
                        "{case:?} {}: {} channels, expected {expected_channels}",
                        file.name, probe["streams"][0]["channels"]
                    ));
                    continue;
                }
                let mut channels = decode(path, decode_rate, expected_channels, opus);
                // The fixture carries a bext start, which `checked_export_record`
                // (written for start-less fixtures) refuses; only the start
                // is read here.
                let record: serde_json::Value =
                    serde_json::from_slice(&fs::read(manifest::manifest_path_for(path)).unwrap()).unwrap();
                let output_rate = match case.format {
                    "opus" | "sgmca" => 48_000,
                    _ => case.rate,
                };

                // AAC codes 1024-sample frames; the M4A edit list states the
                // exact length and FFmpeg decodes the last frame's padding
                // after it. Every other format decodes to its exact length.
                if case.format == "m4a" {
                    let Some(stated) = stated_samples(path) else {
                        problems.push(format!("{case:?} {}: no stated M4A length", file.name));
                        continue;
                    };
                    let decoded = channels[0].len() as i64;
                    if !(stated..stated + 2048).contains(&decoded) {
                        problems.push(format!(
                            "{case:?} {}: decoded {decoded} samples, M4A states {stated}",
                            file.name
                        ));
                    }
                    for channel in &mut channels {
                        channel.truncate(stated.max(0) as usize);
                    }
                }

                let out_len = channels[0].len();
                let removed = full_len as i64 - out_len as i64;
                // What the record says trim removed, at the source's 48 kHz.
                let recorded_trim = record["processing"]["leadingTrim"]["samples"].as_u64();
                let trim = recorded_trim.unwrap_or(0) as usize;
                if trimmed {
                    // Trim removes exactly the leading samples at or below
                    // -50 dBFS on every channel (the digital silence plus the
                    // tone's first samples), and the record says how many.
                    let expected = first_sample_above_threshold(&[0, 1, 2, 3]);
                    if recorded_trim != Some(expected as u64)
                        || record["processing"]["leadingTrim"]["sampleRate"] != RATE as u64
                    {
                        problems.push(format!(
                            "{case:?} {}: record says trim removed {} at {}, expected {expected} at {RATE}",
                            file.name,
                            record["processing"]["leadingTrim"]["samples"],
                            record["processing"]["leadingTrim"]["sampleRate"]
                        ));
                    }
                    // At 48 kHz the output is shorter by exactly that. After
                    // resampling, and in AAC's stated length, it is shorter by
                    // that many source samples' worth, to the nearest sample.
                    let allowed = if output_rate == RATE && case.format != "m4a" {
                        0
                    } else {
                        1
                    };
                    if (removed - scale(trim) as i64).abs() > allowed {
                        problems.push(format!(
                            "{case:?} {}: output is {removed} samples shorter, the record says {trim} at {RATE} Hz",
                            file.name
                        ));
                    }
                } else {
                    if removed != 0 {
                        problems.push(format!(
                            "{case:?} {}: {out_len} samples at {decode_rate} Hz, source has {full_len}",
                            file.name
                        ));
                    }
                    if !record["processing"]["leadingTrim"].is_null() {
                        problems.push(format!("{case:?} {}: a trim is recorded with trim off", file.name));
                    }
                }
                // The output's start is the source start plus the trim, at the
                // output rate, and a WAV carries it in its own bext chunk.
                let start = ((TIME_REFERENCE + trim as u64) * output_rate as u64 + RATE as u64 / 2) / RATE as u64;
                if record["startTime"]["outputTimeReferenceSamples"] != start {
                    problems.push(format!(
                        "{case:?} {}: record start {} (expected {start})",
                        file.name, record["startTime"]["outputTimeReferenceSamples"]
                    ));
                }
                if case.format == "wav" {
                    let carried = manifest::parse_probe(&ffprobe_json(path, &["-show_format", "-show_streams"]))
                        .timing
                        .map(|timing| timing.time_reference_samples);
                    if carried != Some(start) || record["startTime"]["writtenToOutput"] != true {
                        problems.push(format!(
                            "{case:?} {}: the WAV carries start {carried:?} (expected {start})",
                            file.name
                        ));
                    }
                }

                // After resampling, a trim of whole 48 kHz samples can end
                // between output samples.
                let tolerance = if trimmed && decode_rate != RATE { 1 } else { 0 };
                for (channel_index, channel) in channels.iter().enumerate() {
                    // A fade-out covers the last click. Each output click,
                    // moved by the recorded trim, is on its source sample.
                    let clicks = if case.options.fade { &centers[..2] } else { &centers[..] };
                    let lag = click_lag(
                        &reference,
                        channel,
                        clicks,
                        -(scale(trim) as i64),
                        (decode_rate / 40) as i64,
                    );
                    if lag.abs() > tolerance {
                        problems.push(format!(
                            "{case:?} {} channel {channel_index}: clicks are {lag} samples off the source",
                            file.name
                        ));
                    }
                    // The click 300 samples before the end is the loudest
                    // sample of the last 1000 unless a fade-out covers it.
                    if !case.options.fade && channel.len() > scale(1000) {
                        let tail_start = channel.len() - scale(1000);
                        let (peak, _) = channel[tail_start..]
                            .iter()
                            .enumerate()
                            .fold(
                                (0, 0.0_f32),
                                |best, (i, v)| if v.abs() > best.1 { (i, v.abs()) } else { best },
                            );
                        let expected = centers[2] as i64 - removed - tail_start as i64;
                        if (peak as i64 - expected).abs() > tolerance + 1 {
                            problems.push(format!(
                                "{case:?} {} channel {channel_index}: last click at {peak}, expected {expected} (of the last {} samples)",
                                file.name,
                                scale(1000)
                            ));
                        }
                    }
                }
            }
            problems
        }

        #[test]
        fn every_mode_option_format_and_rate_lines_up_sample_for_sample() {
            if !court_fixture_tools_available() {
                return;
            }
            let dir = ConversionTestDir::new();
            let fixture = write_fixture(&dir.0);
            let cases = cases();
            let next = AtomicUsize::new(0);
            let problems = Mutex::new(Vec::new());
            let workers = std::thread::available_parallelism().map_or(2, |n| n.get().min(8));
            std::thread::scope(|scope| {
                for _ in 0..workers {
                    scope.spawn(|| loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(case) = cases.get(index) else { break };
                        let found = check_case(case, &fixture, &dir.0, index);
                        let _ = fs::remove_dir_all(dir.0.join(format!("case_{index}")));
                        if !found.is_empty() {
                            problems.lock().unwrap().push(found.join("\n"));
                        }
                    });
                }
            });
            let problems = problems.into_inner().unwrap();
            assert!(
                problems.is_empty(),
                "{} of {} conversions are misaligned:\n{}",
                problems.len(),
                cases.len(),
                problems.join("\n")
            );
        }

        /// A DigitalCAT `.dm` reaches FFmpeg as a temporary WAV in the work
        /// folder, so its conversions get the same alignment checks: a mono
        /// 48 kHz PCM `.dm` holding channel 0 of the fixture converts in every
        /// mode, plain, trimmed, faded and with everything but auto-level on,
        /// with its clicks on the source's samples and its length kept unless
        /// trim removed the leading silence.
        #[test]
        fn a_dm_source_lines_up_sample_for_sample() {
            if !court_fixture_tools_available() {
                return;
            }
            let dir = ConversionTestDir::new();
            let format = crate::dm::tests::pcm_format(RATE as u32);
            // The block rule for 16-bit mono at 48 kHz: 12000 frames.
            let block_bytes = 24_000;
            let pcm: Vec<u8> = (0..LEN)
                .flat_map(|n| ((fixture_sample(n, 0) * 32767.0).round() as i16).to_le_bytes())
                .collect();
            let t0: u32 = 1_763_800_000;
            let layout: Vec<(u32, &[u8])> = pcm.chunks(block_bytes).map(|block| (t0, block)).collect();
            let dm = dir.0.join("alignment_session.dm");
            fs::write(&dm, crate::dm::tests::dm_bytes(&format, block_bytes as u32, &layout)).unwrap();
            let reference: Vec<f32> = (0..LEN)
                .map(|n| ((fixture_sample(n, 0) * 32767.0).round() / 32768.0) as f32)
                .collect();

            let mut problems = Vec::new();
            let mut index = 0;
            for mode in ["keep", "stereo", "split"] {
                for format in ["wav", "mp3"] {
                    for bits in [0, 2, 4, 31] {
                        let options = Options::from_bits(bits);
                        let out = dir.0.join(format!("dm_case_{index}"));
                        index += 1;
                        fs::create_dir(&out).unwrap();
                        let mut job = court_job(&dm, &out, mode, format);
                        job.labels = vec!["Room".into()];
                        job.chan_vols = vec![1.0];
                        job.rate = RATE.to_string();
                        job.normalize = options.normalize;
                        job.trim = options.trim;
                        job.fade = options.fade;
                        job.hpf = options.hpf;
                        job.declip = options.declip;
                        let case = format!("dm {mode} {format} {options:?}");
                        let conversion = match convert_with_system_ffmpeg(&job, None) {
                            Ok(conversion) => conversion,
                            Err(error) => {
                                problems.push(format!("{case}: conversion failed: {error}"));
                                continue;
                            }
                        };
                        if conversion.feed == dm || conversion.feed.exists() {
                            problems.push(format!("{case}: the feed was not a removed temporary WAV"));
                        }
                        let expected_channels = if mode == "stereo" { 2 } else { 1 };
                        for file in &conversion.files {
                            let channels = decode(Path::new(&file.path), RATE, expected_channels, false);
                            let out_len = channels[0].len();
                            let removed = LEN as i64 - out_len as i64;
                            let trimmed = options.trim && mode != "split";
                            let record: serde_json::Value = serde_json::from_slice(
                                &fs::read(manifest::manifest_path_for(Path::new(&file.path))).unwrap(),
                            )
                            .unwrap();
                            let recorded = record["processing"]["leadingTrim"]["samples"].as_u64().unwrap_or(0);
                            let expected = if trimmed {
                                first_sample_above_threshold(&[0]) as i64
                            } else {
                                0
                            };
                            if removed != expected || recorded as i64 != expected {
                                problems.push(format!(
                                    "{case}: {removed} samples removed, {recorded} recorded, expected {expected}"
                                ));
                            }
                            let clicks = if options.fade { &CLICKS[..2] } else { &CLICKS[..] };
                            for (channel_index, channel) in channels.iter().enumerate() {
                                let lag =
                                    click_lag(&reference, channel, clicks, -(recorded as i64), (RATE / 40) as i64);
                                if lag != 0 {
                                    problems.push(format!(
                                        "{case} channel {channel_index}: clicks are {lag} samples off the .dm"
                                    ));
                                }
                            }
                        }
                        let _ = fs::remove_dir_all(&out);
                    }
                }
            }
            assert!(problems.is_empty(), "{}", problems.join("\n"));
        }

        /// The plain statement of what Trim silence promises: in Keep
        /// Original and Mix to Stereo WAV, each click's output position plus
        /// the recorded trim is its source position, and the output's bext
        /// start is the source start plus the recorded trim.
        #[test]
        fn trimmed_clicks_plus_the_recorded_trim_are_the_source_positions() {
            if !court_fixture_tools_available() {
                return;
            }
            let dir = ConversionTestDir::new();
            let fixture = write_fixture(&dir.0);
            let expected_trim = first_sample_above_threshold(&[0, 1, 2, 3]);
            assert!(expected_trim > LEAD_SILENCE && expected_trim < CLICKS[0]);
            for (index, mode) in ["keep", "stereo"].into_iter().enumerate() {
                let out = dir.0.join(format!("trim_{index}"));
                fs::create_dir(&out).unwrap();
                let mut job = court_job(&fixture, &out, mode, "wav");
                job.rate = RATE.to_string();
                job.trim = true;
                let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
                let path = Path::new(&conversion.files[0].path);
                let record: serde_json::Value =
                    serde_json::from_slice(&fs::read(manifest::manifest_path_for(path)).unwrap()).unwrap();
                let trim = &record["processing"]["leadingTrim"];
                assert_eq!(trim["samples"], expected_trim as u64, "{mode}");
                assert_eq!(trim["sampleRate"], RATE as u64, "{mode}");
                assert_eq!(trim["seconds"], expected_trim as f64 / RATE as f64, "{mode}");
                assert_eq!(trim["decodedSourceSamples"], LEN as u64, "{mode}");
                let channels = decode(path, RATE, if mode == "keep" { CHANNELS } else { 2 }, false);
                assert_eq!(channels[0].len(), LEN - expected_trim, "{mode}");
                for channel in &channels {
                    for &click in &CLICKS {
                        // The click is the loudest sample within 50 ms of it.
                        let at = click - expected_trim;
                        let peak = (at - RATE / 20..(at + RATE / 20).min(channel.len()))
                            .max_by(|&a, &b| channel[a].abs().total_cmp(&channel[b].abs()))
                            .unwrap();
                        assert_eq!(peak + expected_trim, click, "{mode}");
                    }
                }
                let start = TIME_REFERENCE + expected_trim as u64;
                assert_eq!(record["startTime"]["outputTimeReferenceSamples"], start, "{mode}");
                assert_eq!(
                    record["startTime"]["sourceTimeReferenceSamples"], TIME_REFERENCE,
                    "{mode}"
                );
                let carried = manifest::parse_probe(&ffprobe_json(path, &["-show_format", "-show_streams"])).timing;
                assert_eq!(
                    carried.map(|timing| timing.time_reference_samples),
                    Some(start),
                    "{mode}"
                );
                // The trim is the first thing in the chain, by sample count.
                let chain = record["processing"]["filterChain"].as_str().unwrap();
                assert!(
                    chain.contains(&format!(
                        "asetpts=N/SR/TB,atrim=start_sample={expected_trim},asetpts=PTS-STARTPTS"
                    )),
                    "{chain}"
                );
                assert!(!chain.contains("silenceremove"), "{chain}");
            }
        }

        /// A video whose audio track starts 0.5 s after its picture: M4A and
        /// Opus outputs keep that start (as an unprocessed conversion does),
        /// moved later by exactly the recorded trim, and Fade places its
        /// fade-in and fade-out on the audio's own samples, not 0.5 s early.
        #[test]
        fn a_late_audio_track_keeps_its_start_and_its_fades_with_trim_and_fade() {
            if !court_fixture_tools_available() {
                return;
            }
            let dir = ConversionTestDir::new();
            let source = dir.0.join("late_audio.mkv");
            // 0.2 s of silence, then a 440 Hz tone that starts at full level
            // (cosine), 2.5 s in all; the audio track starts at 0.5 s.
            run_ffmpeg(
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "color=black:s=64x64:r=10:d=3",
                    "-itsoffset",
                    "0.5",
                    "-f",
                    "lavfi",
                    "-i",
                    "aevalsrc=if(gte(n\\,9600)\\,0.25*cos(2*PI*440*t)\\,0):s=48000:d=2.5:c=stereo",
                    "-map",
                    "0:v",
                    "-map",
                    "1:a",
                    "-c:v",
                    "mpeg4",
                    "-c:a",
                    "pcm_s16le",
                    "-y",
                ]
                .map(String::from)
                .into_iter()
                .chain([source.to_string_lossy().to_string()])
                .collect::<Vec<_>>(),
            );
            let start_of = |path: &Path| -> f64 {
                let probe = ffprobe_json(path, &["-show_streams", "-select_streams", "a:0"]);
                probe["streams"][0]["start_time"].as_str().unwrap().parse().unwrap()
            };
            assert_eq!(start_of(&source), 0.5);
            let convert = |name: &str, mode: &str, format: &str, trim: bool, fade: bool| {
                let out = dir.0.join(name);
                fs::create_dir(&out).unwrap();
                let mut job = court_job(&source, &out, mode, format);
                job.labels = vec!["Left".into(), "Right".into()];
                job.chan_vols = vec![1.0; 2];
                job.rate = "48000".into();
                job.trim = trim;
                job.fade = fade;
                job.fade_dur = 0.5;
                let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
                PathBuf::from(&conversion.files[0].path)
            };

            for format in ["m4a", "opus"] {
                let plain = start_of(&convert(&format!("plain_{format}"), "keep", format, false, false));
                assert!(
                    plain > 0.4,
                    "{format}: an unprocessed conversion keeps the offset ({plain})"
                );
                let trimmed_path = convert(&format!("trim_{format}"), "keep", format, true, false);
                let record: serde_json::Value =
                    serde_json::from_slice(&fs::read(manifest::manifest_path_for(&trimmed_path)).unwrap()).unwrap();
                assert_eq!(record["processing"]["leadingTrim"]["samples"], 9600, "{format}");
                let trimmed = start_of(&trimmed_path);
                assert!(
                    (trimmed - (plain + 0.2)).abs() < 1e-5,
                    "{format}: trimmed start {trimmed}, unprocessed start {plain} plus the 0.2 s trim"
                );
            }

            // Fade on the stereo mix: compared with the same mix without Fade,
            // the middle is unchanged, the fade-in quiets the first audible
            // moments and the fade-out runs over the last 0.5 s, not 0.5 s
            // early with the end silenced.
            let reference = decode(&convert("nofade", "stereo", "wav", false, false), 48_000, 2, false);
            let faded = decode(&convert("fade", "stereo", "wav", false, true), 48_000, 2, false);
            assert_eq!(faded[0].len(), reference[0].len());
            let rms = |samples: &[f32], from: f64, to: f64| -> f64 {
                let window = &samples[(from * 48_000.0) as usize..(to * 48_000.0) as usize];
                (window.iter().map(|&x| f64::from(x) * f64::from(x)).sum::<f64>() / window.len() as f64).sqrt()
            };
            for channel in 0..2 {
                let (r, f) = (&reference[channel], &faded[channel]);
                let middle = (0.6 * 48_000.0) as usize..(1.9 * 48_000.0) as usize;
                let worst = middle.map(|i| (r[i] - f[i]).abs()).fold(0.0_f32, f32::max);
                assert!(worst < 1e-4, "channel {channel}: the middle changed by {worst}");
                assert!(
                    rms(f, 0.2, 0.4) < 0.8 * rms(r, 0.2, 0.4),
                    "channel {channel}: no fade-in"
                );
                let end = rms(f, 2.05, 2.25) / rms(r, 2.05, 2.25);
                assert!(end > 0.3 && end < 0.95, "channel {channel}: fade-out level {end}");
            }
        }

        /// Trim silence on a recording that is silent from start to end is
        /// refused, and nothing is written.
        #[test]
        fn an_all_silent_recording_is_not_trimmed_to_nothing() {
            if !court_fixture_tools_available() {
                return;
            }
            let dir = ConversionTestDir::new();
            let silent = dir.0.join("silent.wav");
            run_ffmpeg(&[
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                "anullsrc=r=48000:cl=stereo:d=1".into(),
                "-c:a".into(),
                "pcm_s16le".into(),
                "-y".into(),
                silent.to_string_lossy().to_string(),
            ]);
            let out = dir.0.join("out");
            fs::create_dir(&out).unwrap();
            let mut job = court_job(&silent, &out, "stereo", "wav");
            job.trim = true;
            let error = convert_with_system_ffmpeg(&job, None).err().unwrap();
            assert!(
                error.contains("every sample of this recording is below the silence threshold"),
                "{error}"
            );
            assert_eq!(fs::read_dir(&out).unwrap().count(), 0);
        }

        /// An FTR recording on the split-channel route (the route DepoAudio
        /// itself reads: four ADTS channel files joined with `amerge`) gets
        /// the same checks: four 16 kHz AAC channels carrying clicks convert
        /// in every mode, plain, trimmed, faded and with everything but
        /// auto-level on, with the clicks on the samples FFmpeg decodes from
        /// the channel's own AAC, the decoded length kept, and trim removing
        /// only what lies before the first sound.
        #[test]
        fn an_ftr_source_on_the_split_route_lines_up_sample_for_sample() {
            use crate::trm::tests as trm_fixture;
            if !court_fixture_tools_available() || !trm_fixture::aac_tools_available() {
                return;
            }
            const FTR_RATE: usize = 16_000;
            const FTR_LEN: usize = 3 * FTR_RATE;
            const FTR_LEAD: usize = FTR_RATE / 2;
            const FTR_CLICKS: [usize; 3] = [20_005, 33_335, FTR_LEN - 100];
            let sample = |n: usize, channel: usize| -> f64 {
                if FTR_CLICKS.contains(&n) {
                    0.9
                } else if n < FTR_LEAD {
                    0.0
                } else {
                    let near_click = FTR_CLICKS.iter().any(|&click| n.abs_diff(click) <= FTR_RATE / 20);
                    let level = if near_click { 0.005 } else { 0.05 };
                    let frequency = 300.0 + 110.0 * channel as f64;
                    level * (2.0 * std::f64::consts::PI * frequency * (n - FTR_LEAD) as f64 / FTR_RATE as f64).sin()
                }
            };
            let dir = ConversionTestDir::new();
            let mut channel_frames = Vec::new();
            let mut reference = Vec::new();
            for channel in 0..CHANNELS {
                let wav = dir.0.join(format!("ftr_ch{channel}.wav"));
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: FTR_RATE as u32,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut writer = hound::WavWriter::create(&wav, spec).unwrap();
                for n in 0..FTR_LEN {
                    writer
                        .write_sample((sample(n, channel) * 32767.0).round() as i16)
                        .unwrap();
                }
                writer.finalize().unwrap();
                let aac = dir.0.join(format!("ftr_ch{channel}.aac"));
                run_ffmpeg(&[
                    "-i".into(),
                    wav.to_string_lossy().to_string(),
                    "-c:a".into(),
                    "aac".into(),
                    "-b:a".into(),
                    "96k".into(),
                    "-f".into(),
                    "adts".into(),
                    "-y".into(),
                    aac.to_string_lossy().to_string(),
                ]);
                if channel == 0 {
                    // What FFmpeg decodes from channel 0's own AAC: the
                    // conversion must keep exactly these samples' timing.
                    reference = trm_fixture::decode_f32(&[
                        "-f".into(),
                        "aac".into(),
                        "-i".into(),
                        aac.to_string_lossy().to_string(),
                    ]);
                }
                channel_frames.push(trm_fixture::split_adts_13(&fs::read(&aac).unwrap()));
                fs::remove_file(&wav).unwrap();
                fs::remove_file(&aac).unwrap();
            }
            let (stream, lengths) = trm_fixture::ftr_frame_stream(&channel_frames);
            let packets = trm_fixture::round_packets(&stream, &lengths, CHANNELS, 1);
            let trm = dir.0.join("alignment_session.trm");
            fs::write(&trm, trm_fixture::build_avi(&trm_fixture::AviSpec::closed(4), &packets)).unwrap();

            // Where each click lands in the decoded AAC (after the encoder's
            // priming), and the first sample a -50 dBFS trim may not remove.
            let centers: Vec<usize> = FTR_CLICKS
                .iter()
                .map(|&click| {
                    let end = (click + 2048).min(reference.len());
                    (click..end)
                        .max_by(|&a, &b| reference[a].abs().total_cmp(&reference[b].abs()))
                        .unwrap()
                })
                .collect();
            let threshold = 10f32.powf(-50.0 / 20.0);
            let first_sound = reference.iter().position(|v| v.abs() >= threshold).unwrap() as i64;

            let mut problems = Vec::new();
            let mut index = 0;
            for mode in ["keep", "stereo", "split"] {
                for format in ["wav", "flac"] {
                    for bits in [0, 2, 4, 31] {
                        let options = Options::from_bits(bits);
                        let out = dir.0.join(format!("ftr_case_{index}"));
                        index += 1;
                        fs::create_dir(&out).unwrap();
                        let mut job = court_job(&trm, &out, mode, format);
                        job.rate = FTR_RATE.to_string();
                        job.normalize = options.normalize;
                        job.trim = options.trim;
                        job.fade = options.fade;
                        job.hpf = options.hpf;
                        job.declip = options.declip;
                        let case = format!("ftr {mode} {format} {options:?}");
                        let conversion = match convert_with_system_ffmpeg(&job, None) {
                            Ok(conversion) => conversion,
                            Err(error) => {
                                problems.push(format!("{case}: conversion failed: {error}"));
                                continue;
                            }
                        };
                        if conversion.feed_inputs.len() != CHANNELS
                            || conversion.feed_inputs.iter().any(|input| Path::new(input).exists())
                        {
                            problems.push(format!(
                                "{case}: not read as {CHANNELS} removed channel files: {:?}",
                                conversion.feed_inputs
                            ));
                        }
                        let expected_channels = match mode {
                            "keep" => CHANNELS,
                            "stereo" => 2,
                            _ => 1,
                        };
                        let expected_outputs = if mode == "split" { CHANNELS } else { 1 };
                        if conversion.files.len() != expected_outputs {
                            problems.push(format!("{case}: {} outputs", conversion.files.len()));
                        }
                        for file in &conversion.files {
                            let channels = decode(Path::new(&file.path), FTR_RATE, expected_channels, false);
                            let out_len = channels[0].len();
                            let removed = reference.len() as i64 - out_len as i64;
                            let trimmed = options.trim && mode != "split";
                            let record: serde_json::Value = serde_json::from_slice(
                                &fs::read(manifest::manifest_path_for(Path::new(&file.path))).unwrap(),
                            )
                            .unwrap();
                            let chain = record["processing"]["filterChain"].as_str().unwrap_or_default();
                            if trimmed {
                                if !(1..=first_sound + 1).contains(&removed) {
                                    problems.push(format!(
                                        "{case} {}: trim removed {removed} samples (first sound at {first_sound})",
                                        file.name
                                    ));
                                }
                                // Exactly the recorded count, cut after the
                                // channel files are joined.
                                if record["processing"]["leadingTrim"]["samples"] != removed {
                                    problems.push(format!(
                                        "{case} {}: removed {removed} samples, the record says {}",
                                        file.name, record["processing"]["leadingTrim"]
                                    ));
                                }
                                let merge_at = chain.find("amerge=inputs=");
                                let trim_at = chain.find(&format!("atrim=start_sample={removed}"));
                                if !matches!((merge_at, trim_at), (Some(merge), Some(trim)) if merge < trim)
                                    || chain.contains("silenceremove")
                                {
                                    problems.push(format!("{case} {}: chain {chain}", file.name));
                                }
                            } else if removed != 0 {
                                problems.push(format!(
                                    "{case} {}: {out_len} samples, the recording decodes to {}",
                                    file.name,
                                    reference.len()
                                ));
                            }
                            // Trim cuts a counted number of samples, so the
                            // clicks stay exactly on their samples.
                            let tolerance = 0;
                            let clicks = if options.fade { &centers[..2] } else { &centers[..] };
                            for (channel_index, channel) in channels.iter().enumerate() {
                                let lag = click_lag(&reference, channel, clicks, -removed, (FTR_RATE / 40) as i64);
                                if lag.abs() > tolerance {
                                    problems.push(format!(
                                        "{case} {} channel {channel_index}: clicks are {lag} samples off the recording",
                                        file.name
                                    ));
                                }
                            }
                        }
                        let _ = fs::remove_dir_all(&out);
                    }
                }
            }
            assert!(problems.is_empty(), "{}", problems.join("\n"));
        }

        /// An SGMCA source with two recording sessions reaches FFmpeg as a
        /// concat script over its sessions, so its conversions get the same
        /// checks: in every mode, for WAV, MP3, M4A and SGMCA, plain, faded
        /// and with Normalize, Fade, high-pass and de-clip on, every output
        /// holds the joined timeline's 240000 samples with each click on its
        /// sample in its session, and a fade-out is placed from the decoded
        /// length of both sessions (5 s), not from the first or the last
        /// session's own length. Trim is refused where it would apply, with
        /// nothing written.
        #[test]
        fn a_two_session_sgmca_source_lines_up_sample_for_sample() {
            if !court_fixture_tools_available() {
                return;
            }
            let dir = ConversionTestDir::new();
            // Session 1: 3 s, clicks on channel 1 at 0.5 s and 1.5 s and on
            // channel 2 at 1 s. Session 2: 2 s, a click on channel 1 at 0.5 s
            // and on channel 2 at 1.25 s: 3.5 s and 4.25 s into the joined
            // audio, before a 0.5 s fade-out from 4.5 s.
            let first = click_session(&dir.0, "first", 144_000, [&[24_000, 72_000], &[48_000]]);
            let second = click_session(&dir.0, "second", 96_000, [&[24_000], &[60_000]]);
            let sgmca = dir.0.join("alignment_sessions.sgmca");
            fs::write(
                &sgmca,
                crate::sgmca::fixtures::direct_audio(&[
                    ("2026-11-22T10:15:30-0500", &first),
                    ("2026-11-22T10:20:00-0500", &second),
                ]),
            )
            .unwrap();
            const JOINED: usize = 240_000;
            let impulses = |positions: &[usize]| {
                let mut reference = vec![0.0_f32; JOINED];
                for &at in positions {
                    reference[at] = 0.9;
                }
                reference
            };
            let left = [24_000, 72_000, 168_000];
            let right = [48_000, 204_000];
            let both = [24_000, 48_000, 72_000, 168_000, 204_000];

            let mut problems = Vec::new();
            let mut index = 0;
            for mode in ["keep", "stereo", "split"] {
                for format in ["wav", "mp3", "m4a", "sgmca"] {
                    for bits in [0, 4, 29] {
                        let options = Options::from_bits(bits);
                        let out = dir.0.join(format!("sgmca_case_{index}"));
                        index += 1;
                        fs::create_dir(&out).unwrap();
                        let mut job = two_channel_job(&sgmca, &out, mode, format);
                        job.rate = RATE.to_string();
                        job.normalize = options.normalize;
                        job.fade = options.fade;
                        job.hpf = options.hpf;
                        job.declip = options.declip;
                        let case = format!("sgmca {mode} {format} {options:?}");
                        let conversion = match convert_with_system_ffmpeg(&job, None) {
                            Ok(conversion) => conversion,
                            Err(error) => {
                                problems.push(format!("{case}: conversion failed: {error}"));
                                continue;
                            }
                        };
                        if conversion.feed == sgmca || conversion.feed.exists() {
                            problems.push(format!("{case}: the feed was not a removed concat script"));
                        }
                        for (file_index, file) in conversion.files.iter().enumerate() {
                            let path = Path::new(&file.path);
                            let channel_count = if mode == "split" { 1 } else { 2 };
                            let mut channels = decode(path, RATE, channel_count, format == "sgmca");
                            if format == "m4a" {
                                // AAC pads its last frame; the edit list
                                // states the length.
                                let Some(stated) = stated_samples(path) else {
                                    problems.push(format!("{case}: no stated M4A length"));
                                    continue;
                                };
                                for channel in &mut channels {
                                    channel.truncate(stated.max(0) as usize);
                                }
                            }
                            let record: serde_json::Value =
                                serde_json::from_slice(&fs::read(manifest::manifest_path_for(path)).unwrap()).unwrap();
                            if options.fade {
                                let fade = &record["processing"]["fadeOut"];
                                if fade["decodedSourceSeconds"] != 5.0 || fade["startSeconds"] != 4.5 {
                                    problems.push(format!("{case}: fade-out {fade}"));
                                }
                            }
                            if format == "sgmca" {
                                let starts: Vec<_> = record["output"]["sgmca"]["segments"]
                                    .as_array()
                                    .map(|segments| {
                                        segments.iter().map(|segment| segment["startSample"].clone()).collect()
                                    })
                                    .unwrap_or_default();
                                if starts != [0, 144_000] {
                                    problems.push(format!("{case}: output sessions start at {starts:?}"));
                                }
                            }
                            for (channel_index, channel) in channels.iter().enumerate() {
                                let label = format!("{case} {} channel {channel_index}", file.name);
                                if channel.len() != JOINED {
                                    problems.push(format!(
                                        "{label}: {} samples, the sessions hold {JOINED}",
                                        channel.len()
                                    ));
                                    continue;
                                }
                                let source_channel = if mode == "split" { file_index } else { channel_index };
                                let clicks: &[usize] = match (mode, source_channel) {
                                    ("stereo", _) => &both,
                                    (_, 0) => &left,
                                    _ => &right,
                                };
                                let lag = click_lag(&impulses(clicks), channel, clicks, 0, (RATE / 40) as i64);
                                if lag != 0 {
                                    problems.push(format!("{label}: clicks are {lag} samples off the sessions"));
                                }
                                if options.fade {
                                    let peak = |from: usize, to: usize| {
                                        channel[from..to]
                                            .iter()
                                            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
                                    };
                                    // The tail of the second session fades;
                                    // the click at 4.25 s is not touched.
                                    let before = peak(203_000, 205_000);
                                    let tail = peak(JOINED - 1_000, JOINED);
                                    if tail > before * 0.1 {
                                        problems.push(format!("{label}: tail peak {tail}, click {before}"));
                                    }
                                }
                            }
                        }
                        let _ = fs::remove_dir_all(&out);
                    }
                }
            }

            // Trim would move every session's place and start time: refused for Keep Original and Mix to Stereo, with
            // nothing written. Split Channels does not trim (as in the
            // matrix above), so every sample stays in its session.
            for mode in ["keep", "stereo", "split"] {
                let out = dir.0.join(format!("sgmca_trim_{mode}"));
                fs::create_dir(&out).unwrap();
                let mut job = two_channel_job(&sgmca, &out, mode, "wav");
                job.trim = true;
                match (mode, convert_with_system_ffmpeg(&job, None)) {
                    ("split", Ok(conversion)) => {
                        for file in &conversion.files {
                            let length = decode(Path::new(&file.path), RATE, 1, false)[0].len();
                            if length != JOINED {
                                problems.push(format!("sgmca trim split {}: {length} samples", file.name));
                            }
                        }
                    }
                    (_, Err(error)) if mode != "split" && error.contains("2 recording sessions") => {
                        if fs::read_dir(&out).unwrap().count() != 0 {
                            problems.push(format!("sgmca trim {mode}: files left"));
                        }
                    }
                    (_, other) => problems.push(format!("sgmca trim {mode}: {:?}", other.map(|c| c.files))),
                }
            }
            assert!(problems.is_empty(), "{}", problems.join("\n"));
        }

        /// A Liberty / BIS DCR reaches FFmpeg as a temporary WAV in the work
        /// folder that DepoAudio writes from the recording's atoms, so its
        /// conversions get the same checks as a `.dm`: a 2-channel 48 kHz
        /// 16-bit PCM DCR holding channels 0 and 1 of the fixture converts in
        /// every mode, plain, trimmed, faded and with everything but
        /// auto-level on, with its clicks on the source's samples and its
        /// length kept unless trim removed the leading silence.
        #[test]
        fn a_dcr_source_lines_up_sample_for_sample() {
            if !court_fixture_tools_available() {
                return;
            }
            let dir = ConversionTestDir::new();
            let names = ["Judge", "Witness"];
            let mut builder = DcrBuilder::new(crate::dcr::tests::pcm16_format(RATE as u32), 0b11, &names);
            builder.marker(b"audi#mrk");
            let sample =
                |n: usize, channel: usize| ((fixture_sample(n, channel) * 32767.0).round() as i16).to_le_bytes();
            for n in 0..LEN {
                builder.atom(&[&sample(n, 0), &sample(n, 1)]);
            }
            let dcr = builder.write(&dir.0, "alignment_hearing.dcr");
            let reference: Vec<f32> = (0..LEN)
                .map(|n| ((fixture_sample(n, 0) * 32767.0).round() / 32768.0) as f32)
                .collect();

            let mut problems = Vec::new();
            let mut index = 0;
            for mode in ["keep", "stereo", "split"] {
                for format in ["wav", "mp3"] {
                    for bits in [0, 2, 4, 31] {
                        let options = Options::from_bits(bits);
                        let out = dir.0.join(format!("dcr_case_{index}"));
                        index += 1;
                        fs::create_dir(&out).unwrap();
                        let mut job = court_job(&dcr, &out, mode, format);
                        job.chan_vols = vec![1.0; 2];
                        job.rate = RATE.to_string();
                        job.normalize = options.normalize;
                        job.trim = options.trim;
                        job.fade = options.fade;
                        job.hpf = options.hpf;
                        job.declip = options.declip;
                        let case = format!("dcr {mode} {format} {options:?}");
                        let conversion = match convert_with_system_ffmpeg(&job, None) {
                            Ok(conversion) => conversion,
                            Err(error) => {
                                problems.push(format!("{case}: conversion failed: {error}"));
                                continue;
                            }
                        };
                        if conversion.feed == dcr || conversion.feed.exists() {
                            problems.push(format!("{case}: the feed was not a removed temporary WAV"));
                        }
                        let expected_channels = if mode == "split" { 1 } else { 2 };
                        let expected_files = if mode == "split" { 2 } else { 1 };
                        if conversion.files.len() != expected_files {
                            problems.push(format!("{case}: {} outputs", conversion.files.len()));
                        }
                        for file in &conversion.files {
                            let channels = decode(Path::new(&file.path), RATE, expected_channels, false);
                            let out_len = channels[0].len();
                            let removed = LEN as i64 - out_len as i64;
                            let trimmed = options.trim && mode != "split";
                            let record: serde_json::Value = serde_json::from_slice(
                                &fs::read(manifest::manifest_path_for(Path::new(&file.path))).unwrap(),
                            )
                            .unwrap();
                            let recorded = record["processing"]["leadingTrim"]["samples"].as_u64().unwrap_or(0);
                            // Trim removes exactly the leading samples at or
                            // below the threshold on both channels, and the
                            // record says how many.
                            let expected = if trimmed {
                                first_sample_above_threshold(&[0, 1]) as i64
                            } else {
                                0
                            };
                            if removed != expected || recorded as i64 != expected {
                                problems.push(format!(
                                    "{case}: {removed} samples removed, {recorded} recorded, expected {expected} (the .dcr holds {LEN})"
                                ));
                            }
                            let clicks = if options.fade { &CLICKS[..2] } else { &CLICKS[..] };
                            for (channel_index, channel) in channels.iter().enumerate() {
                                let lag =
                                    click_lag(&reference, channel, clicks, -(recorded as i64), (RATE / 40) as i64);
                                if lag != 0 {
                                    problems.push(format!(
                                        "{case} channel {channel_index}: clicks are {lag} samples off the .dcr"
                                    ));
                                }
                            }
                        }
                        let _ = fs::remove_dir_all(&out);
                    }
                }
            }
            assert!(problems.is_empty(), "{}", problems.join("\n"));
        }

        /// A DCR whose codec FFmpeg decodes one channel per stream (here IMA
        /// ADPCM) is joined by an extra FFmpeg run (`amerge`) before
        /// conversion. Each output channel must be exactly that channel's own
        /// decoded audio, sample for sample: the join adds, removes and moves
        /// nothing.
        #[test]
        fn a_merged_ima_adpcm_dcr_lines_up_with_each_channel_decoded_alone() {
            if !court_fixture_tools_available() {
                return;
            }
            let dir = ConversionTestDir::new();
            let mut tones = Vec::new();
            let mut alone = Vec::new();
            for channel in 0..2 {
                let raw = dir.0.join(format!("ima_raw_{channel}.wav"));
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: RATE as u32,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut writer = hound::WavWriter::create(&raw, spec).unwrap();
                for n in 0..LEN {
                    writer
                        .write_sample((fixture_sample(n, channel) * 32767.0).round() as i16)
                        .unwrap();
                }
                writer.finalize().unwrap();
                let coded = dir.0.join(format!("ima_{channel}.wav"));
                run_ffmpeg(&[
                    "-i".into(),
                    raw.to_string_lossy().to_string(),
                    "-c:a".into(),
                    "adpcm_ima_wav".into(),
                    "-y".into(),
                    coded.to_string_lossy().to_string(),
                ]);
                alone.push(decode(&coded, RATE, 1, false).remove(0));
                let bytes = fs::read(&coded).unwrap();
                let (mut at, mut fmt) = (12, Vec::new());
                let data = loop {
                    let id = &bytes[at..at + 4];
                    let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
                    if id == b"fmt " {
                        fmt = bytes[at + 8..at + 8 + size].to_vec();
                    }
                    if id == b"data" {
                        break bytes[at + 8..(at + 8 + size).min(bytes.len())].to_vec();
                    }
                    at += 8 + size + size % 2;
                };
                tones.push((fmt, data));
            }
            let dcr = coded_dcr(&dir.0, "alignment_ima.dcr", &tones, None);
            let mut problems = Vec::new();
            for mode in ["keep", "split"] {
                let out = dir.0.join(format!("ima_{mode}"));
                fs::create_dir(&out).unwrap();
                let mut job = court_job(&dcr, &out, mode, "wav");
                job.chan_vols = vec![1.0; 2];
                job.rate = RATE.to_string();
                let conversion = convert_with_system_ffmpeg(&job, None).unwrap();
                if conversion.feed.exists() {
                    problems.push(format!("ima {mode}: the merged feed was left behind"));
                }
                if !conversion.feed.starts_with(crate::workdir::tests::test_root()) {
                    problems.push(format!(
                        "ima {mode}: the merged feed was not in the private work folder"
                    ));
                }
                let channels: Vec<Vec<f32>> = if mode == "keep" {
                    decode(Path::new(&conversion.files[0].path), RATE, 2, false)
                } else {
                    conversion
                        .files
                        .iter()
                        .map(|file| decode(Path::new(&file.path), RATE, 1, false).remove(0))
                        .collect()
                };
                if channels.len() != 2 {
                    problems.push(format!("ima {mode}: {} channels", channels.len()));
                }
                for (index, (channel, reference)) in channels.iter().zip(&alone).enumerate() {
                    if channel.len() != reference.len() {
                        problems.push(format!(
                            "ima {mode} channel {index}: {} samples, decoded alone {}",
                            channel.len(),
                            reference.len()
                        ));
                    }
                    // A 16-bit WAV at the source rate with nothing turned on is
                    // lossless here, so every sample must match. (IMA ADPCM
                    // smears a click over tens of samples, too wide for the
                    // click correlation above to place exactly.)
                    if let Some(at) = channel.iter().zip(reference).position(|(a, b)| a != b) {
                        problems.push(format!(
                            "ima {mode} channel {index}: differs from its own decode at sample {at}"
                        ));
                    }
                }
            }
            assert!(problems.is_empty(), "{}", problems.join("\n"));
        }

        #[test]
        fn the_stereo_peak_limiter_compensates_its_lookahead() {
            assert!(STEREO_PEAK_LIMITER.split(':').any(|option| option == "latency=1"));
        }
    }
}
