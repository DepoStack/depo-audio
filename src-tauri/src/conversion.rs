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
    basename, detect_format_for_path, input_codec_args, output_args, output_ext, prepare_audio_feed_with_cancel,
    safe_ffmpeg_input_prelude, safe_label,
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

/// Stereo downmix: a unity-gain SUM of all channels on both L and R.
/// `vols` are per-channel volumes already multiplied by 1/num_ch; missing
/// entries fall back to an equal-weight mix.
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
    format!("pan=stereo|c0={}|c1={},volume={:.1}", mix, mix, scale)
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
    let split_str = format!("[0:a]asplit={}[{}]", num_ch, sp_tags.join("]["));
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

fn validate_finite_range(label: &str, value: f64, min: f64, max: f64) -> Result<(), String> {
    if !value.is_finite() || !(min..=max).contains(&value) {
        return Err(format!("{label} must be a finite value from {min} to {max}."));
    }
    Ok(())
}

fn validate_convert_request(job: &ConvertJob) -> Result<(), String> {
    if job.denoise || job.enhance || job.dereverb {
        return Err(
            "Learned-model processing is not included in DepoAudio v1.0.3. Turn off Noise Removal, Enhance Clarity, and Reduce Room Echo."
                .into(),
        );
    }
    if !matches!(job.mode.as_str(), "stereo" | "keep" | "split") {
        return Err(format!("Unsupported conversion mode: {}", job.mode));
    }
    if !matches!(job.format.as_str(), "wav" | "mp3" | "flac" | "opus" | "m4a") {
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

    let fmt = detect_format_for_path(&job.src_path).ok_or("Unrecognised file format")?;

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
/// temporary Ogg copy; every other format is used as is) and fingerprint the
/// original file as the person selected it, before any preparation, for the
/// export record (DA-08). The temporary copy, if any, lives as long as the
/// returned guard.
fn prepare_source(
    src: &Path,
    cancelled: &(dyn Fn() -> bool + Send + Sync),
) -> Result<(PathBuf, Option<TempFile>, FileRecord), String> {
    let (feed, guard) = prepare_audio_feed_with_cancel(src, Some(cancelled))?;
    let (sha256, size_bytes) = manifest::sha256_file(src, cancelled)?;
    let source = FileRecord {
        file_name: basename(&src.to_string_lossy()),
        sha256,
        size_bytes,
    };
    Ok((feed, guard, source))
}

/// What the probes found before the FFmpeg command is planned.
struct ConversionProbes {
    /// Channel count of the input; probed only for Mix to Stereo and Split.
    channels: Option<u32>,
    /// Per-channel auto-level gains, when auto-level analysis ran.
    channel_gains: Option<Vec<f64>>,
    /// Shared processing filters (de-clip, HPF, loudnorm, fade, trim).
    proc: Vec<String>,
    /// Channel count, rate and start time for the export record.
    probed: manifest::ProbedSource,
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
    start_shifted: bool,
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
    let out_codec = output_args(&job.format, &job.rate, job.mp3_bitrate);

    // Start time (DA-08): read the source's BWF bext TimeReference and carry
    // it into WAV outputs, rescaled to the output rate, unless leading-silence
    // trim moved the first sample. Every other output has the copied
    // `time_reference` tag removed, so only the export record holds the start.
    let proc = probes.proc.clone();
    let start_shifted = manifest::filters_shift_start(&proc);
    let output_rate = manifest::output_sample_rate(&out_codec);
    let mut encoder_args = out_codec;
    encoder_args.extend(manifest::start_time_metadata_args(
        &job.format,
        probes.probed.timing,
        output_rate,
        start_shifted,
    ));

    let mut ffmpeg_args = safe_ffmpeg_input_prelude();
    ffmpeg_args.extend(input_codec);
    ffmpeg_args.extend(["-i".into(), feed.to_string_lossy().to_string()]);

    let planned: Vec<PlannedOutput>;
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
                all.push("alimiter=limit=0.97:level=false".into());
            }
            planned = reserve_output_paths([(out_dir.join(format!("{}{}", base, ext)), None, None)])?;
            track_planned(cleanup, &planned);
            let dst = planned[0].path.clone();
            let chain = all.join(",");
            args.extend(["-af".into(), chain.clone()]);
            args.extend(encoder_args.clone());
            args.extend(["-y".into(), dst.to_string_lossy().to_string()]);
            filter_option = Some("-af");
            filter_chain = Some(chain);
        }

        "keep" => {
            planned = reserve_output_paths([(out_dir.join(format!("{}_orig{}", base, ext)), None, None)])?;
            track_planned(cleanup, &planned);
            let dst = planned[0].path.clone();
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
            for (i, output) in planned.iter().enumerate() {
                args.extend(["-map".into(), format!("[op{}]", i)]);
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
        start_shifted,
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
    output: &PlannedOutput,
    file: &OutputFile,
    sha256: String,
    size_bytes: u64,
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
        source_channel_index: output.channel_index,
        label: output.label.clone(),
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
    for (output, file) in plan.planned.iter().zip(&files) {
        let hashed = manifest::sha256_file(&output.path, &|| cancel.cancelled());
        cancel.check()?;
        let (sha256, size_bytes) = hashed?;
        let record = manifest::build_manifest(input, output_facts(job, plan, output, file, sha256, size_bytes));
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

    // Stereo injects per-channel gains in its pan filter and Split injects
    // them after isolating each channel. Keep Original rejects auto-leveling
    // before work starts because one shared gain would not balance channels.
    let proc = build_proc_filters_with_gain(app, job, effective_feed, None, Some(&is_cancelled)).await;
    cancel.check()?;

    let probed = probe_manifest_source_cancellable(app, effective_feed, Some(&is_cancelled)).await;
    cancel.check()?;

    // Mix to Stereo and Split need the channel count; Keep Original does not.
    let channels = if matches!(job.mode.as_str(), "stereo" | "split") {
        let channels = probe_channels_cancellable(app, effective_feed, Some(&is_cancelled)).await;
        cancel.check()?;
        channels
    } else {
        None
    };

    let probes = ConversionProbes {
        channels,
        channel_gains,
        proc,
        probed,
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
    let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
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
            shifted: plan.start_shifted,
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
            "pan=stereo|c0=0.2500*c0+0.2500*c1+0.2500*c2+0.2500*c3|c1=0.2500*c0+0.2500*c1+0.2500*c2+0.2500*c3,volume=4.0"
        );
    }

    #[test]
    fn stereo_pan_missing_vols_fall_back_to_equal_weight() {
        assert_eq!(
            stereo_pan_filter(2, &[]),
            "pan=stereo|c0=0.5000*c0+0.5000*c1|c1=0.5000*c0+0.5000*c1,volume=2.0"
        );
    }

    #[test]
    fn stereo_pan_carries_per_channel_gains() {
        // Auto-level: gains 2.0 and 0.5 pre-multiplied by the 1/2 weight
        assert_eq!(
            stereo_pan_filter(2, &[1.0, 0.25]),
            "pan=stereo|c0=1.0000*c0+0.2500*c1|c1=1.0000*c0+0.2500*c1,volume=2.0"
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
            "[0:a]asplit=2[sp0][sp1];[sp0]pan=mono|c0=c0[op0];[sp1]pan=mono|c0=c1[op1]"
        );
    }

    #[test]
    fn split_filter_appends_shared_proc_chain_per_channel() {
        let proc = vec!["highpass=f=80".to_string(), "loudnorm=I=-16:TP=-1.5:LRA=11".to_string()];
        assert_eq!(
            split_filter_complex(2, false, None, &proc),
            "[0:a]asplit=2[sp0][sp1];[sp0]pan=mono|c0=c0,highpass=f=80,loudnorm=I=-16:TP=-1.5:LRA=11[op0];[sp1]pan=mono|c0=c1,highpass=f=80,loudnorm=I=-16:TP=-1.5:LRA=11[op1]"
        );
    }

    #[test]
    fn split_filter_injects_each_channels_own_gain() {
        let gains = vec![2.0, 1.0, 0.5];
        assert_eq!(
            split_filter_complex(3, true, Some(&gains), &[]),
            "[0:a]asplit=3[sp0][sp1][sp2];[sp0]pan=mono|c0=c0,volume=2.0000[op0];[sp1]pan=mono|c0=c1[op1];[sp2]pan=mono|c0=c2,volume=0.5000[op2]"
        );
    }

    #[test]
    fn split_filter_ignores_gains_when_auto_level_off() {
        let gains = vec![2.0, 0.5];
        assert_eq!(
            split_filter_complex(2, false, Some(&gains), &[]),
            "[0:a]asplit=2[sp0][sp1];[sp0]pan=mono|c0=c0[op0];[sp1]pan=mono|c0=c1[op1]"
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
            assert!(error.contains("DepoAudio v1.0.3"));
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
        assert!(error.contains("DepoAudio v1.0.3"));
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

    /// A synthetic SGMCA: the made-up prefix followed by a 4-channel Ogg Opus
    /// stream. Returns the .sgmca path and the bare Ogg bytes.
    fn synthetic_sgmca(dir: &Path) -> (PathBuf, Vec<u8>) {
        let ogg = dir.join("synthetic_stream.ogg");
        synthetic_four_channel(
            &ogg,
            &["-c:a", "libopus", "-mapping_family", "1", "-b:a", "128k", "-f", "ogg"],
        );
        let stream = fs::read(&ogg).unwrap();
        assert_eq!(&stream[..4], b"OggS");
        fs::remove_file(&ogg).unwrap();
        let sgmca = dir.join("synthetic_session.sgmca");
        let mut bytes = synthetic_vendor_prefix();
        bytes.extend_from_slice(&stream);
        fs::write(&sgmca, bytes).unwrap();
        (sgmca, stream)
    }

    /// A synthetic DigitalCAT `.dm`: DepoAudio hands `.dm` files to FFmpeg
    /// unchanged, so a plain 4-channel WAV under that name exercises the path.
    fn synthetic_dm(dir: &Path) -> PathBuf {
        let wav = dir.join("synthetic_stream.wav");
        synthetic_four_channel(&wav, &["-c:a", "pcm_s16le", "-f", "wav"]);
        let dm = dir.join("synthetic_session.dm");
        fs::rename(&wav, &dm).unwrap();
        dm
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
        let format = detect_format_for_path(&job.src_path).ok_or("Unrecognised file format")?;
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
        // As build_proc_filters_with_gain: the duration only places the fade-out.
        let duration = if job.fade {
            crate::ffmpeg::parse_duration_probe(&probe(crate::ffmpeg::duration_probe_args(&feed)))
        } else {
            None
        };
        let probes = ConversionProbes {
            channels: crate::ffmpeg::parse_channel_probe(&probe(crate::ffmpeg::channel_probe_args(&feed))),
            channel_gains,
            proc: crate::ffmpeg::proc_filters(job, None, duration),
            probed: serde_json::from_slice::<serde_json::Value>(&probe(crate::ffmpeg::manifest_probe_args(&feed)))
                .map(|value| manifest::parse_probe(&value))
                .unwrap_or_default(),
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
            shifted: plan.start_shifted,
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

    #[test]
    fn synthetic_digitalcat_dm_converts_as_a_plain_container() {
        if !court_fixture_tools_available() {
            return;
        }
        let dir = ConversionTestDir::new();
        let dm = synthetic_dm(&dir.0);
        let split_out = dir.0.join("split");
        let mix_out = dir.0.join("mix");
        fs::create_dir(&split_out).unwrap();
        fs::create_dir(&mix_out).unwrap();

        let split = convert_with_system_ffmpeg(&court_job(&dm, &split_out, "split", "wav"), None).unwrap();
        assert_eq!(split.feed, dm, "a .dm file is passed to FFmpeg unchanged");
        assert_split_outputs(&split, &dm, ".wav");

        let mix = convert_with_system_ffmpeg(&court_job(&dm, &mix_out, "stereo", "mp3"), None).unwrap();
        assert_mix_output(&mix, &dm, "synthetic_session.mp3");
        assert!(dm.exists());
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
}
