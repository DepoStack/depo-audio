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
    basename, detect_format_for_path, input_codec_args, output_args, output_ext, prepare_audio_feed_cancellable,
    safe_ffmpeg_input_prelude, safe_label,
};
use crate::manifest::{self, FileRecord, ManifestInput, OutputFacts};
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
    let preparation_cancelled: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || preparation_cancel.cancelled());
    let prepared = prepare_audio_feed_cancellable(src.to_path_buf(), preparation_cancelled).await;
    cancel.check()?;
    let (feed_path, _feed_guard) = prepared?;

    // Fingerprint the original file as the person selected it, before any
    // format preparation, for the export record (DA-08).
    let source = hash_file_cancellable(src.to_path_buf(), cancel).await?;
    let source = FileRecord {
        file_name: basename(&job.src_path),
        sha256: source.0,
        size_bytes: source.1,
    };

    do_convert_inner(app, job, &feed_path, max_bytes, cancel, source).await
}

async fn hash_file_cancellable(path: PathBuf, cancel: &ConversionCancel) -> Result<(String, u64), String> {
    let hash_cancel = cancel.clone();
    let result =
        tauri::async_runtime::spawn_blocking(move || manifest::sha256_file(&path, &|| hash_cancel.cancelled()))
            .await
            .map_err(|error| format!("Could not fingerprint the file: {error}"))?;
    cancel.check()?;
    result
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

    // Start time (DA-08): read the source's BWF bext TimeReference and carry
    // it into WAV outputs, rescaled to the output rate, unless leading-silence
    // trim moved the first sample. Every other output has the copied
    // `time_reference` tag removed, so only the export record holds the start.
    let probed = probe_manifest_source_cancellable(app, effective_feed, Some(&is_cancelled)).await;
    cancel.check()?;
    let start_shifted = manifest::filters_shift_start(&proc);
    let output_rate = manifest::output_sample_rate(&out_codec);
    let mut encoder_args = out_codec.clone();
    encoder_args.extend(manifest::start_time_metadata_args(
        &job.format,
        probed.timing,
        output_rate,
        start_shifted,
    ));

    let mut ffmpeg_args = safe_ffmpeg_input_prelude();
    ffmpeg_args.extend(input_codec.clone());
    ffmpeg_args.extend(["-i".into(), effective_feed.to_string_lossy().to_string()]);

    let mut output_cleanup = OutputCleanup::default();
    let planned: Vec<PlannedOutput>;
    let filter_option: Option<&'static str>;
    let filter_chain: Option<String>;

    match job.mode.as_str() {
        "stereo" => {
            let num_ch = probe_channels_cancellable(app, effective_feed, Some(&is_cancelled)).await;
            cancel.check()?;
            let num_ch = num_ch.ok_or("Cannot create stereo mix: unable to determine the input's channel count")?;
            let num_ch = validate_conversion_channels(num_ch)?;
            let weight = 1.0 / num_ch as f64;

            // Use auto-level gains if available, otherwise use manual chan_vols.
            // Gains are only trustworthy when the analysis saw the same channel
            // count we're mixing — its probe can fall back to 1 (or cap at 16)
            // on files this probe still reads fine, and a mismatched vector
            // would boost some channels and silence others.
            let vols: Vec<f64> = match channel_gains {
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
            track_planned(&mut output_cleanup, &planned);
            let dst = planned[0].path.clone();
            let chain = all.join(",");
            let mut args = ffmpeg_args.clone();
            args.extend(["-af".into(), chain.clone()]);
            args.extend(encoder_args.clone());
            args.extend(["-y".into(), dst.to_string_lossy().to_string()]);
            filter_option = Some("-af");
            filter_chain = Some(chain);
            run_ffmpeg_with_timeout(
                app,
                args,
                &job.id,
                job.ffmpeg_timeout as u64,
                total_secs,
                Some(&is_cancelled),
            )
            .await?;
            cancel.check()?;
        }

        "keep" => {
            planned = reserve_output_paths([(out_dir.join(format!("{}_orig{}", base, ext)), None, None)])?;
            track_planned(&mut output_cleanup, &planned);
            let dst = planned[0].path.clone();
            let mut args = ffmpeg_args.clone();
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
            run_ffmpeg_with_timeout(
                app,
                args,
                &job.id,
                job.ffmpeg_timeout as u64,
                total_secs,
                Some(&is_cancelled),
            )
            .await?;
            cancel.check()?;
        }

        "split" => {
            let num_ch = probe_channels_cancellable(app, effective_feed, Some(&is_cancelled)).await;
            cancel.check()?;
            let num_ch = num_ch.ok_or("Cannot split channels: unable to determine the input's channel count")?;
            let num_ch = validate_conversion_channels(num_ch)?;
            let labels = split_labels(num_ch, &job.labels);
            planned = reserve_output_paths(labels.iter().enumerate().map(|(index, label)| {
                (
                    out_dir.join(format!("{}_{}{}", base, label, ext)),
                    Some(index as u32),
                    job.labels.get(index).cloned(),
                )
            }))?;
            track_planned(&mut output_cleanup, &planned);

            // Use asplit + pan=mono per channel instead of channelsplit:
            // channelsplit requires the actual channel layout (defaulting to
            // stereo), which breaks mono and 4-channel court recordings.
            let mut args = ffmpeg_args.clone();
            // Same channel-count guard as the stereo arm: a gains vector from
            // a desynced analysis probe must not be applied per-channel
            let valid_gains = channel_gains.as_ref().filter(|g| g.len() == num_ch as usize);
            let fc = split_filter_complex(num_ch, job.auto_level, valid_gains, &proc);
            args.extend(["-filter_complex".into(), fc.clone(), "-y".into()]);
            for (i, output) in planned.iter().enumerate() {
                args.extend(["-map".into(), format!("[op{}]", i)]);
                args.extend(encoder_args.clone());
                args.push(output.path.to_string_lossy().to_string());
            }
            filter_option = Some("-filter_complex");
            filter_chain = Some(fc);
            run_ffmpeg_with_timeout(
                app,
                args,
                &job.id,
                job.ffmpeg_timeout as u64,
                total_secs,
                Some(&is_cancelled),
            )
            .await?;
            cancel.check()?;
        }

        _ => return Err(format!("Unknown mode: {}", job.mode)),
    }

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
        // OutputCleanup removes this empty file and every valid sibling.
        return Err(format!("Output file is empty: {}", empty.name));
    }

    // Export record (DA-08). A failure here fails the conversion and removes
    // every output, so no file is delivered without its record.
    let manifest_input = ManifestInput {
        app_version: app.package_info().version.to_string(),
        created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        source,
        probed: &probed,
        labels: &job.labels,
        shifted: start_shifted,
    };
    for (output, file) in planned.iter().zip(&files) {
        let (sha256, size_bytes) = hash_file_cancellable(output.path.clone(), cancel).await?;
        let record = manifest::build_manifest(
            &manifest_input,
            OutputFacts {
                file_name: file.name.clone(),
                sha256,
                size_bytes,
                format: &job.format,
                mode: &job.mode,
                filter_option,
                filter_chain: filter_chain.clone(),
                encoder_args: encoder_args.clone(),
                source_channel_index: output.channel_index,
                label: output.label.clone(),
            },
        );
        manifest::write_manifest(&output.manifest, &record).map_err(|error| {
            format!(
                "The audio was converted, but its export record could not be saved, so the output was removed: {error}"
            )
        })?;
    }

    cancel.check()?;
    output_cleanup.commit();

    Ok(files)
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
}
