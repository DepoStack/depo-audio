use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Emitter};

use crate::ffmpeg::{
    build_proc_filters_with_gain, ensure_ftr_decoder, probe_channels_cancellable, probe_duration_cancellable,
    probe_manifest_source_cancellable, run_ffmpeg_with_timeout,
};
use crate::helpers::{
    basename, input_codec_args, output_args, output_ext, prepare_audio_feed_detailed, safe_ffmpeg_input_prelude,
    safe_label, source_audio_map_args, wav_estimated_bytes, wav_rf64_args,
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

/// Opus bitrate per mono stream of an SGMCA output: wideband speech at
/// 16 kHz is clear well below this.
const SGMCA_KBPS_PER_CHANNEL: u32 = 32;

/// Peak guard for Mix to Stereo with normalize off. FFmpeg's alimiter looks
/// 5 ms ahead and, by default (`latency=0`), outputs that lookahead as a
/// delay: the output keeps the source's length, so the audio starts about
/// 5 ms late and the last 5 ms of the source are cut. `latency=1` removes the
/// delay so the output lines up sample for sample with the source. The option
/// is in every bundled FFmpeg (5.1 or newer); an FFmpeg without it fails the
/// conversion instead of shifting the audio.
const STEREO_PEAK_LIMITER: &str = "alimiter=limit=0.97:level=false:latency=1";

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
fn split_filter_complex(num_ch: u32, auto_level: bool, channel_gains: Option<&Vec<f64>>, proc: &[String]) -> String {
    let sp_tags: Vec<String> = (0..num_ch as usize).map(|i| format!("sp{}", i)).collect();
    // `[0:a:0]`: the converted stream, never FFmpeg's choice among tracks.
    let split_str = format!(
        "{}asplit={}[{}]",
        crate::helpers::source_audio_filter_input(),
        num_ch,
        sp_tags.join("][")
    );
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
    if fmt.handler == "ftr" {
        ensure_ftr_decoder(app).await?;
        cancel.check()?;
    }
    let preparation_cancel = cancel.clone();
    let preparation_src = src.to_path_buf();
    let prepared = tauri::async_runtime::spawn_blocking(move || {
        prepare_source(&preparation_src, &move || preparation_cancel.cancelled())
    })
    .await
    .map_err(|error| format!("Audio preparation task failed: {error}"))?;
    cancel.check()?;
    let (feed_path, _feed_guard, source) = prepared?;

    do_convert_inner(app, job, &feed_path, max_bytes, cancel, source).await
}

/// Prepare the selected file for FFmpeg (an SGMCA loses its vendor prefix in a
/// temporary Ogg copy, a `.dm` has its audio read into a temporary WAV; every
/// other format is used as is) and fingerprint the
/// original file as the person selected it, before any preparation, for the
/// export record (DA-08). The temporary copy, if any, lives as long as the
/// returned guard.
fn prepare_source(
    src: &Path,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(PathBuf, Option<TempFile>, FileRecord), String> {
    let prepared = prepare_audio_feed_detailed(src, Some(cancelled))?;
    let (sha256, size_bytes) = manifest::sha256_file(src, cancelled)?;
    let source = FileRecord {
        file_name: basename(&src.to_string_lossy()),
        sha256,
        size_bytes,
        dm_timeline: prepared.dm_timeline,
    };
    Ok((prepared.path, prepared.guard, source))
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
    fade_out: Option<manifest::FadeOutRecord>,
    leading_trim: Option<manifest::LeadingTrimRecord>,
}

/// Pure core of a conversion once the input has been probed: reserve every
/// output with its export record (tracked by `cleanup` as soon as reserved)
/// and build the exact FFmpeg arguments. No process runs here, so tests can
/// drive the same plan with FFmpeg from PATH.
fn plan_conversion(
    job: &ConvertJob,
    feed: &Path,
    probes: &ConversionProbes,
    cleanup: &mut OutputCleanup,
) -> Result<ConversionPlan, String> {
    let input_codec = input_codec_args(feed);

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

    let mut ffmpeg_args = safe_ffmpeg_input_prelude();
    ffmpeg_args.extend(input_codec);
    ffmpeg_args.extend(["-i".into(), feed.to_string_lossy().to_string()]);

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
            map_args = vec![source_audio_map_args()];
            args.extend(map_args[0].clone());
            args.extend(["-af".into(), chain.clone()]);
            args.extend(encoder_args.clone());
            args.extend(["-y".into(), dst.to_string_lossy().to_string()]);
            filter_option = Some("-af");
            filter_chain = Some(chain);
        }

        "keep" => {
            validate_keep_channels(&job.format, probes.channels)?;
            planned = reserve_output_paths([(out_dir.join(format!("{}_orig{}", base, ext)), None, None)])?;
            track_planned(cleanup, &planned);
            let dst = planned[0].path.clone();
            map_args = vec![source_audio_map_args()];
            args.extend(map_args[0].clone());
            if proc.is_empty() {
                filter_option = None;
                filter_chain = None;
            } else {
                let chain = proc.join(",");
                args.extend(["-af".into(), chain.clone()]);
                filter_option = Some("-af");
                filter_chain = Some(chain);
            }
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
            let fc = split_filter_complex(num_ch, job.auto_level, valid_gains, &proc);
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

    Ok(ConversionPlan {
        planned,
        args,
        filter_option,
        filter_chain,
        encoder_args,
        map_args,
        fade_out: probes.fade_out.clone(),
        leading_trim: probes.leading_trim.clone(),
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
    let files = collect_outputs(&plan.planned)?;
    if job.format == "sgmca" {
        // Every SGMCA output must really be Direct Audio: a sound Ogg Opus
        // stream with one uncoupled mono stream per channel.
        for file in &files {
            let cancelled = || cancel.cancelled();
            crate::sgmca::verify_written(Path::new(&file.path), Some(&cancelled)).map_err(|error| {
                format!(
                    "{} failed the SGMCA format check, so the output was removed: {error}",
                    file.name
                )
            })?;
        }
    }
    for (index, (output, file)) in plan.planned.iter().zip(&files).enumerate() {
        let hashed = manifest::sha256_file(&output.path, &|| cancel.cancelled());
        cancel.check()?;
        let (sha256, size_bytes) = hashed?;
        // Which header the WAV muxer wrote, read from the file itself.
        let wav_header = if job.format == "wav" {
            Some(manifest::wav_header_kind(&output.path)?)
        } else {
            None
        };
        let record = manifest::build_manifest(
            input,
            output_facts(job, plan, index, file, sha256, size_bytes, wav_header),
        );
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
    feed: &Path,
    max_input_size: u64,
    cancel: &ConversionCancel,
    source: FileRecord,
) -> Result<Vec<OutputFile>, String> {
    cancel.check()?;
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
            feed.to_string_lossy().to_string(),
            cancel.epoch.clone(),
            cancel.generation,
        );
        let gain_result =
            crate::analysis::analyze_channel_gains(app, &feed.to_string_lossy(), max_input_size, Some(&analysis_ctx))
                .await;
        cancel.check()?;
        channel_gains = Some(gain_result.map_err(|error| format!("Auto-level analysis failed: {error}"))?);
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

    let probes = ConversionProbes {
        channels,
        channel_gains,
        proc: built.filters,
        fade_out: built.fade_out,
        leading_trim: built.leading_trim,
        probed,
        duration_secs: total_secs,
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
    .await?;
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
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// A made-up vendor prefix. It only has to be non-Ogg bytes in front of the
    /// stream, inside the 8 KiB region the SGMCA handler scans.
    fn synthetic_vendor_prefix() -> Vec<u8> {
        let mut prefix = b"DEPOAUDIO SYNTHETIC TEST PREFIX - NOT A REAL SGMCA HEADER\n".to_vec();
        prefix.resize(1024, 0x5a);
        assert!(!prefix.windows(4).any(|window| window == b"OggS"));
        prefix
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
    /// `SYNTHETIC_CHANNEL_VOLUMES`, written with `encoder` to `path`.
    fn synthetic_four_channel(path: &Path, encoder: &[&str]) {
        let mut args: Vec<String> = Vec::new();
        for frequency in [220, 330, 440, 550] {
            args.extend([
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                format!("sine=frequency={frequency}:duration={SYNTHETIC_SECONDS}:sample_rate=48000"),
            ]);
        }
        let levels: Vec<String> = SYNTHETIC_CHANNEL_VOLUMES
            .iter()
            .enumerate()
            .map(|(index, volume)| format!("[{index}:a]volume={volume}[v{index}]"))
            .collect();
        let graph = format!(
            "{};[v0][v1][v2][v3]join=inputs=4:channel_layout=quad[out]",
            levels.join(";")
        );
        args.extend(["-filter_complex".into(), graph, "-map".into(), "[out]".into()]);
        args.extend(encoder.iter().map(|arg| arg.to_string()));
        args.extend(["-y".into(), path.to_string_lossy().to_string()]);
        run_ffmpeg(&args);
    }

    /// A synthetic 4-channel Ogg Opus stream in the SGMCA layout: mapping
    /// family 255, one uncoupled mono stream per channel, 16 kHz input tag.
    fn synthetic_direct_audio(path: &Path) {
        synthetic_four_channel(
            path,
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

    /// A synthetic SGMCA with the made-up prefix in front of its stream (some
    /// exports carry one). Returns the .sgmca path and the bare Ogg bytes.
    fn synthetic_sgmca(dir: &Path) -> (PathBuf, Vec<u8>) {
        let ogg = dir.join("synthetic_stream.ogg");
        synthetic_direct_audio(&ogg);
        let stream = fs::read(&ogg).unwrap();
        assert_eq!(&stream[..4], b"OggS");
        fs::remove_file(&ogg).unwrap();
        let sgmca = dir.join("synthetic_session.sgmca");
        let mut bytes = synthetic_vendor_prefix();
        bytes.extend_from_slice(&stream);
        fs::write(&sgmca, bytes).unwrap();
        (sgmca, stream)
    }

    /// A synthetic SGMCA as the format describes it: the Ogg stream from byte
    /// 0, no prefix.
    fn synthetic_plain_sgmca(dir: &Path) -> PathBuf {
        let sgmca = dir.join("synthetic_session.sgmca");
        synthetic_direct_audio(&sgmca);
        sgmca
    }

    /// Interleaved 32-bit float PCM at 48 kHz, decoded by FFmpeg.
    fn decode_f32(path: &Path, channels: usize) -> Vec<Vec<f32>> {
        let output = std::process::Command::new("ffmpeg")
            .args([
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-c:a",
                "libopus",
                "-i",
            ])
            .arg(path)
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
        let output = std::process::Command::new("ffprobe")
            .args(["-v", "quiet", "-print_format", "json"])
            .args(args)
            .arg(path)
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
        let output = std::process::Command::new("ffmpeg")
            .args(["-nostdin", "-hide_banner", "-i"])
            .arg(path)
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
    struct CourtConversion {
        files: Vec<OutputFile>,
        feed: PathBuf,
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
        validate_convert_request(job)?;
        let format = crate::helpers::detect_format_for_path(&job.src_path).ok_or("Unrecognised file format")?;
        assert!(
            !matches!(format.handler.as_str(), "rejected" | "ftr"),
            "this harness covers container formats only"
        );
        let (feed, _feed_guard, source) = prepare_source(Path::new(&job.src_path), &|| false)?;

        let probe = |args: Vec<String>| {
            let output = std::process::Command::new("ffprobe").args(args).output().unwrap();
            assert!(output.status.success(), "ffprobe failed");
            output.stdout
        };
        // As do_convert_inner: the duration is always probed (progress and the
        // WAV size estimate); build_proc_filters_with_gain uses it only to
        // place the fade-out.
        let total_secs = crate::ffmpeg::parse_duration_probe(&probe(crate::ffmpeg::duration_probe_args(&feed)));
        let probed = serde_json::from_slice::<serde_json::Value>(&probe(crate::ffmpeg::manifest_probe_args(&feed)))
            .map(|value| manifest::parse_probe(&value))
            .unwrap_or_default();
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
        let probes = ConversionProbes {
            channels: crate::ffmpeg::parse_channel_probe(&probe(crate::ffmpeg::channel_probe_args(&feed))),
            channel_gains,
            proc: built.filters,
            fade_out: built.fade_out,
            leading_trim: built.leading_trim,
            probed,
            duration_secs: total_secs,
        };

        let mut output_cleanup = OutputCleanup::default();
        let plan = plan_conversion(job, &feed, &probes, &mut output_cleanup)?;
        let mut args = plan.args.clone();
        args.splice(0..0, ["-loglevel".to_string(), "error".to_string()]);
        let status = std::process::Command::new("ffmpeg").args(&args).status().unwrap();
        if !status.success() {
            return Err(format!("ffmpeg exited with {status}"));
        }

        let input = ManifestInput {
            app_version: env!("CARGO_PKG_VERSION").into(),
            created_at: "2026-11-13T09:00:00Z".into(),
            source,
            probed: &probes.probed,
            labels: &job.labels,
        };
        let cancel = ConversionCancel::new(Arc::new(AtomicU64::new(1)), 1);
        let files = finish_outputs(job, &plan, &input, output_cleanup, &cancel)?;
        Ok(CourtConversion { files, feed })
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
        // A synthetic source carries no bext start, so none is claimed.
        assert_eq!(record["startTime"]["known"], false);
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

        // The record fingerprints the .sgmca as selected, not the stripped
        // Ogg feed; the original is untouched; the temporary feed is gone.
        let record = checked_export_record(&conversion.files[0], &sgmca);
        let stream_digest = {
            let bare = dir.0.join("bare_stream.ogg");
            fs::write(&bare, &stream).unwrap();
            sha256_of(&bare)
        };
        assert_ne!(record["source"]["sha256"], stream_digest);
        assert_eq!(fs::read(&sgmca).unwrap(), original);
        assert_ne!(conversion.feed, sgmca);
        assert!(!conversion.feed.exists(), "the stripped Ogg feed was left behind");
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
        assert!(!conversion.feed.exists(), "the stripped Ogg feed was left behind");
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
            .err()
            .expect("a file with no Ogg stream must not convert");
        assert!(error.contains("does not contain an Ogg audio stream"), "{error}");
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
        let error = crate::helpers::prepare_audio_feed_with_cancel(&dm, None).unwrap_err();
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
        let mut args: Vec<String> = Vec::new();
        for frequency in [440, 220, 330, 550, 660] {
            args.extend([
                "-f".into(),
                "lavfi".into(),
                "-i".into(),
                format!("sine=frequency={frequency}:duration={SYNTHETIC_SECONDS}:sample_rate=48000"),
            ]);
        }
        let graph = "[0:a]volume=0.5[floor];[1:a]volume=0.0625[q0];[2:a]volume=0.0625[q1];\
                     [3:a]volume=0.0625[q2];[4:a]volume=0.0625[q3];\
                     [q0][q1][q2][q3]join=inputs=4:channel_layout=quad[interp]";
        args.extend(["-filter_complex".into(), graph.into()]);
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
        };
        let feed_arg = feed.to_string_lossy().to_string();
        for mode in ["stereo", "keep"] {
            let mut job = valid_job();
            job.src_path = feed_arg.clone();
            job.out_dir = dir.0.to_string_lossy().to_string();
            job.mode = mode.into();
            let mut cleanup = OutputCleanup::default();
            let plan = plan_conversion(&job, &feed, &probes, &mut cleanup).unwrap();
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
        let plan = plan_conversion(&job, &feed, &probes, &mut cleanup).unwrap();
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
            };
            let mut cleanup = OutputCleanup::default();
            plan_conversion(&job, &feed, &probes, &mut cleanup).unwrap()
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
        };
        let mut cleanup = OutputCleanup::default();
        let plan = plan_conversion(&job, &source, &probes, &mut cleanup).unwrap();
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
            "afade=t=out:st=2.000:d=2,afade=t=in:d=2"
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

    /// FFmpeg-backed: an SGMCA file with two recording sessions (each its own
    /// Ogg stream, with a segment header in front) is refused before any
    /// output is written, by its sessions' starts and lengths. Converted as
    /// one stream, the second session would start right where the first ends.
    #[test]
    fn a_two_session_sgmca_is_refused_before_any_output() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let ogg = dir.0.join("synthetic_stream.ogg");
        synthetic_direct_audio(&ogg);
        let stream = fs::read(&ogg).unwrap();
        fs::remove_file(&ogg).unwrap();
        let bytes = crate::sgmca::fixtures::segmented(&[
            ("2026-11-22T10:15:30-0500", &stream),
            ("2026-11-22T10:20:00-0500", &stream),
        ]);
        let source = dir.0.join("synthetic_two_sessions.sgmca");
        fs::write(&source, bytes).unwrap();
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();
        for (mode, format) in [("keep", "wav"), ("stereo", "sgmca"), ("split", "flac")] {
            let mut job = court_job(&source, &out, mode, format);
            job.fade = true;
            let error = convert_with_system_ffmpeg(&job, None).err().expect("refused");
            assert!(error.contains("2 recording sessions"), "{error}");
            assert!(error.contains("2026-11-22T10:20:00-0500"), "{error}");
            assert_eq!(fs::read_dir(&out).unwrap().count(), 0, "no output or record is left");
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
            let error = convert_with_system_ffmpeg(&job, None).err().expect("refused");
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
    /// sample for sample, and stay within one 20 ms frame of the source.
    /// FFmpeg versions differ in how many trailing samples they keep when
    /// they resample to 16 kHz (FFmpeg 8 drops 768 samples at 48 kHz from a
    /// 2 s tone; 6.1 keeps them all), so an exact 2 s check would test FFmpeg.
    fn assert_sgmca_duration(path: &Path, info: &crate::sgmca::SgmcaInfo) {
        let channels = usize::from(info.head.channels);
        let decoded = decode_f32(path, channels);
        assert!(
            decoded.iter().all(|channel| channel.len() as u64 == info.samples),
            "libopus decoded {:?} samples per channel, the granules say {}",
            decoded.iter().map(Vec::len).collect::<Vec<_>>(),
            info.samples
        );
        assert!((info.duration_seconds - SYNTHETIC_SECONDS).abs() <= 0.02, "{info:?}");
    }

    #[test]
    fn plain_family_255_sgmca_is_read_in_place_and_split() {
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
        assert_eq!(conversion.feed, sgmca, "a plain SGMCA needs no temporary copy");
    }

    #[test]
    fn wav_to_sgmca_writes_one_mono_opus_stream_per_channel_with_a_record() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let wav = dir.0.join("synthetic_session.wav");
        synthetic_four_channel(&wav, &["-c:a", "pcm_s16le", "-f", "wav"]);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&wav, &out, "keep", "sgmca"), None).unwrap();
        assert_eq!(conversion.files.len(), 1);
        let file = &conversion.files[0];
        assert_eq!(file.name, "synthetic_session_orig.sgmca");
        let written = Path::new(&file.path);
        let info = crate::sgmca::verify_written(written, None).unwrap();
        assert_eq!(info.head.channels, 4);
        assert_eq!(info.head.mapping, [0, 1, 2, 3]);
        assert_eq!(info.head.input_sample_rate, 16_000);
        assert_sgmca_duration(written, &info);
        assert_audio(written, 4, "48000");
        let record = checked_export_record(file, &wav);
        assert_eq!(record["output"]["format"], "sgmca");
        assert_eq!(record["output"]["sampleRate"], 16_000);
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
        let sgmca = synthetic_plain_sgmca(&dir.0);
        let out = dir.0.join("out");
        fs::create_dir(&out).unwrap();

        let conversion = convert_with_system_ffmpeg(&court_job(&sgmca, &out, "split", "sgmca"), None).unwrap();
        assert_split_outputs(&conversion, &sgmca, ".sgmca");
        for file in &conversion.files {
            let info = crate::sgmca::verify_written(Path::new(&file.path), None).unwrap();
            assert_eq!((info.head.channels, info.head.stream_count), (1, 1));
        }
    }

    #[test]
    fn sgmca_slice_is_sample_exact_and_matches_the_original_audio() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let sgmca = synthetic_plain_sgmca(&dir.0);
        let cut = dir.0.join("cut.sgmca");
        let report = crate::sgmca::slice(&sgmca, &cut, 0.5, Some(1.5), None).unwrap();
        assert_eq!(report.samples, 48_000);
        assert_eq!(report.channels, 4);

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
        // And the slice is a valid Direct Audio file in its own right.
        assert!(crate::sgmca::verify_written(&cut, None).is_ok());
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
            if opus {
                command.args(["-c:a", "libopus"]);
            }
            let output = command
                .arg("-i")
                .arg(path)
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
        /// format × offered rate (Opus 48 kHz and SGMCA 16 kHz are fixed) ×
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
                ("sgmca", &[16_000]),
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
                    "opus" => 48_000,
                    "sgmca" => 16_000,
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

        #[test]
        fn the_stereo_peak_limiter_compensates_its_lookahead() {
            assert!(STEREO_PEAK_LIMITER.split(':').any(|option| option == "latency=1"));
        }
    }
}
