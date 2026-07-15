use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use regex::Regex;
use tauri::{AppHandle, Emitter};

use crate::ffmpeg::{probe_channels, probe_duration};
use crate::helpers::ffprobe_bin_name;
use crate::types::{AnalysisResult, TurnSegment};

// ── Audio analysis engine ────────────────────────────────────────────────────
//
// Pre-scans audio to detect issues and recommend AI processing.
// Uses FFmpeg/ffprobe for loudness + peak analysis and the Pipecat Smart Turn
// ONNX model for speaker turn detection.

/// Target loudness for auto-leveling (LUFS).
const TARGET_LUFS: f64 = -16.0;
/// Channels quieter than this are considered silent (LUFS).
const SILENCE_THRESHOLD: f64 = -60.0;
/// LUFS spread across channels that triggers auto-leveling recommendation.
const LEVELING_THRESHOLD: f64 = 3.0;
/// Peak dBFS threshold above which clipping is detected.
const CLIPPING_THRESHOLD: f64 = -0.5;
/// Noise floor above which denoising is recommended (dBFS).
const NOISE_THRESHOLD: f64 = -45.0;
/// Sample rate at or below which bandwidth extension is recommended.
const NARROWBAND_RATE: u32 = 16000;
/// Court recorders top out at 16 channels; anything larger is a corrupt
/// header and must not become a loop bound.
const MAX_SCAN_CHANNELS: u32 = 16;
/// Wall-clock budget for the Smart Turn inference pass (all channels).
const TURNS_BUDGET_SECS: u64 = 150;

/// Context for a user-visible Scan: progress events + cancellation.
/// Conversion-time analysis (auto-level) passes None and runs silently.
#[derive(Clone)]
pub(crate) struct ScanCtx {
    pub app: AppHandle,
    pub path: String,
    epoch: Arc<AtomicU64>,
    my_gen: u64,
}

impl ScanCtx {
    pub fn new(app: AppHandle, path: String, epoch: Arc<AtomicU64>) -> Self {
        let my_gen = epoch.load(Ordering::SeqCst);
        Self { app, path, epoch, my_gen }
    }

    /// True once cancel_scan_cmd has bumped the epoch past this scan's start.
    pub fn cancelled(&self) -> bool {
        self.epoch.load(Ordering::Relaxed) != self.my_gen
    }

    pub fn check(&self) -> Result<(), String> {
        if self.cancelled() { Err("Scan cancelled".into()) } else { Ok(()) }
    }

    /// Within-file progress: phase name + estimated fraction complete [0, 1].
    /// `gen` lets the frontend drop trailing events from a cancelled scan
    /// that would otherwise pollute a successor scan of the same file.
    pub fn emit(&self, phase: &str, pct: f64) {
        let _ = self.app.emit(
            "scan:progress",
            serde_json::json!({ "path": self.path, "phase": phase, "pct": pct, "gen": self.my_gen }),
        );
    }
}

fn emit(ctx: Option<&ScanCtx>, phase: &str, pct: f64) {
    if let Some(c) = ctx { c.emit(phase, pct); }
}

fn check(ctx: Option<&ScanCtx>) -> Result<(), String> {
    match ctx { Some(c) => c.check(), None => Ok(()) }
}

/// How often a long-running sidecar pass re-emits its phase so the frontend's
/// stall watchdog can tell "slow but alive" from "wedged".
const HEARTBEAT_SECS: u64 = 10;

/// sidecar_output_opt plus a progress heartbeat and scan cancellation: the
/// per-pass timeout backstops run up to 120s, and without events during the
/// wait the frontend watchdog would cancel a scan the backend was about to
/// recover gracefully. A cancelled scan kills the in-flight decoder within
/// ~1s instead of letting it run out its backstop.
pub(crate) async fn sidecar_with_heartbeat(
    app: &AppHandle,
    bin: &str,
    args: Vec<String>,
    secs: u64,
    ctx: Option<&ScanCtx>,
    phase: &str,
    pct: f64,
) -> Option<crate::ffmpeg::SidecarOutput> {
    let is_cancelled = || ctx.map(|c| c.cancelled()).unwrap_or(false);
    let fut = crate::ffmpeg::sidecar_output_cancellable(app, bin, args, secs, Some(&is_cancelled));
    tokio::pin!(fut);
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(HEARTBEAT_SECS), &mut fut).await {
            Ok(out) => return out,
            Err(_) => emit(ctx, phase, pct),
        }
    }
}

/// Run full audio analysis on a file.
pub(crate) async fn analyze_audio(
    app: &AppHandle,
    path: &str,
    ctx: Option<&ScanCtx>,
) -> Result<AnalysisResult, String> {
    let feed = Path::new(path);
    crate::safety::check_file_safe(feed)?;

    // Formats FFmpeg can't auto-detect (FTR) need a forced input decoder on
    // every decode below — without it each pass burns its full timeout on an
    // undecodable file and the whole scan grinds for nothing.
    let input_codec = crate::helpers::scan_input_codec_args(feed);

    emit(ctx, "probe", 0.02);
    // Probe basic metadata. On probe failure assume ONE channel — inventing
    // phantom channels multiplies every per-channel pass (each with a long
    // timeout backstop) on exactly the files that are already struggling.
    // Cap the count so a corrupt header can't become a loop bound.
    // Emit between the chained probes: three wedged 30s probes back-to-back
    // would otherwise exceed the frontend's stall watchdog.
    let channels = probe_channels(app, feed).await.unwrap_or(1).min(MAX_SCAN_CHANNELS);
    emit(ctx, "probe", 0.03);
    let duration = probe_duration(app, feed).await.unwrap_or(0.0);
    emit(ctx, "probe", 0.04);
    let sample_rate = probe_sample_rate(app, feed).await.unwrap_or(48000);
    check(ctx)?;

    // Run loudness + peak analysis per channel
    let (per_channel_lufs, per_channel_peak, loudness_failures) =
        analyze_loudness_and_peaks(app, feed, channels, &input_codec, ctx).await?;
    check(ctx)?;

    // Detect clipping
    let has_clipping = per_channel_peak.iter().any(|&p| p >= CLIPPING_THRESHOLD);

    // Detect level imbalance (only among active channels)
    let active_lufs: Vec<f64> = per_channel_lufs
        .iter()
        .copied()
        .filter(|&l| l > SILENCE_THRESHOLD)
        .collect();
    let needs_leveling = if active_lufs.len() > 1 {
        let min = active_lufs.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = active_lufs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        (max - min).abs() > LEVELING_THRESHOLD
    } else {
        false
    };

    // Estimate noise floor from quietest channel RMS
    // A rough proxy: if the quietest channel LUFS is above the noise threshold
    // and there's significant content, denoising may help.
    emit(ctx, "noise", 0.28);
    let needs_denoise = estimate_noise_floor(app, feed, &input_codec, ctx).await > NOISE_THRESHOLD;
    check(ctx)?;

    // Narrowband detection
    let is_narrowband = sample_rate <= NARROWBAND_RATE;

    // Compute auto-level gains
    let channel_gains: Vec<f64> = per_channel_lufs
        .iter()
        .map(|&lufs| {
            if lufs <= SILENCE_THRESHOLD {
                1.0 // Leave silent channels alone
            } else {
                let gain = 10_f64.powf((TARGET_LUFS - lufs) / 20.0);
                gain.clamp(0.1, 10.0)
            }
        })
        .collect();

    // Voice activity detection (run early so we can skip expensive steps on silence)
    emit(ctx, "speech", 0.32);
    let (vad_result, file_undecodable) =
        match crate::vad::detect_speech(app, std::path::Path::new(path), ctx).await {
            Ok(v) => (Some(v), false),
            // The FILE failed to decode: every other pass would burn its own
            // decode timeout on the same bytes, so skip them. (The old
            // behavior defaulted speech_ratio to 1.0, force-running every
            // pass on exactly the files that couldn't be analyzed at all.)
            Err(crate::vad::VadError::Undecodable(_)) => (None, true),
            // VAD itself is unavailable (model missing/corrupt, ORT failure):
            // says nothing about the file — run the other passes, their own
            // models may be fine.
            Err(crate::vad::VadError::Unavailable(_)) => (None, false),
        };
    check(ctx)?;

    // Gate the expensive passes on measured speech; with no measurement the
    // gate is open unless the file itself proved undecodable.
    let speech_gate = |threshold: f64| match &vad_result {
        Some(v) => v.speech_ratio > threshold,
        None => !file_undecodable,
    };

    // Smart Turn detection — skip if very little speech detected
    let turns = if speech_gate(0.1) {
        emit(ctx, "turns", 0.45);
        detect_turns(app, feed, channels, &input_codec, ctx).await
    } else {
        Vec::new()
    };
    check(ctx)?;

    // Build recommendations
    let mut recommendations = Vec::new();
    if needs_denoise {
        recommendations.push("Background noise detected — AI denoising recommended".into());
    }
    if needs_leveling {
        let spread = active_lufs.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            - active_lufs.iter().cloned().fold(f64::INFINITY, f64::min);
        recommendations.push(format!(
            "{:.1} dB spread across speakers — auto-leveling recommended",
            spread
        ));
    }
    if has_clipping {
        let clipped: Vec<usize> = per_channel_peak
            .iter()
            .enumerate()
            .filter(|(_, &p)| p >= CLIPPING_THRESHOLD)
            .map(|(i, _)| i + 1)
            .collect();
        recommendations.push(format!(
            "Clipping detected on channel{} {} — de-clipping recommended",
            if clipped.len() > 1 { "s" } else { "" },
            clipped.iter().map(|c| c.to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    if is_narrowband {
        recommendations.push(format!(
            "Narrow-band audio detected ({} Hz) — bandwidth extension recommended",
            sample_rate
        ));
    }

    // VAD was already run above; add recommendation if mostly silence
    if let Some(ref vad) = vad_result {
        if vad.speech_ratio < 0.3 && duration > 10.0 {
            recommendations.push(format!(
                "Only {:.0}% of this recording contains speech — consider trimming silence",
                vad.speech_ratio * 100.0
            ));
        }
    }

    // Quality scoring — skip if no speech detected
    let quality_score = if speech_gate(0.05) {
        emit(ctx, "quality", 0.88);
        crate::scoring::score_quality(app, std::path::Path::new(path), ctx).await
            .map(|qs| crate::types::QualityScoreResult { sig: qs.sig, bak: qs.bak, ovr: qs.ovr })
            .ok()
    } else {
        None
    };
    check(ctx)?;

    // Speaker count detection — skip if no speech detected
    let speaker_count = if speech_gate(0.1) {
        emit(ctx, "speakers", 0.94);
        crate::speakers::detect_speakers(app, std::path::Path::new(path), ctx).await
            .map(|info| info.count)
            .ok()
    } else {
        None
    };
    check(ctx)?;

    // Be honest when the file itself defeated part of the analysis. Total
    // loudness failure already returned Err above, so any failure count here
    // means SOME channels silently read as silence.
    if file_undecodable || loudness_failures > 0 {
        recommendations.push(
            "This file could not be fully decoded for analysis — convert it first, then scan the converted output".into(),
        );
    }

    // Note when AI models are missing so the user knows results may be incomplete
    let mut missing_models = Vec::new();
    if vad_result.is_none() { missing_models.push("VAD"); }
    if quality_score.is_none() { missing_models.push("quality scoring"); }
    if speaker_count.is_none() { missing_models.push("speaker detection"); }
    if !missing_models.is_empty() {
        recommendations.push(format!(
            "Some AI models not available ({}) — results may be incomplete",
            missing_models.join(", ")
        ));
    }

    emit(ctx, "done", 1.0);

    Ok(AnalysisResult {
        channels,
        duration,
        sample_rate,
        per_channel_lufs,
        per_channel_peak,
        has_clipping,
        needs_leveling,
        needs_denoise,
        is_narrowband,
        turns,
        channel_gains,
        recommendations,
        quality_score,
        speaker_count,
        speech_ratio: vad_result.map(|v| v.speech_ratio),
    })
}

// ── Loudness & peak analysis via FFmpeg ──────────────────────────────────────

async fn analyze_loudness_and_peaks(
    app: &AppHandle,
    feed: &Path,
    channels: u32,
    input_codec: &[String],
    ctx: Option<&ScanCtx>,
) -> Result<(Vec<f64>, Vec<f64>, u32), String> {
    let mut lufs_vec = Vec::with_capacity(channels as usize);
    let mut peak_vec = Vec::with_capacity(channels as usize);
    let mut failures = 0u32;

    // A failed channel reads as silence instead of aborting the scan — one
    // bad channel shouldn't discard the loudness of the others. Only if EVERY
    // channel fails is the file genuinely unreadable.
    if channels <= 1 {
        // Mono or single-channel: analyze directly
        emit(ctx, "loudness", 0.05);
        match analyze_single_channel(app, feed, None, input_codec, ctx, 0.05).await {
            Ok((lufs, peak)) => { lufs_vec.push(lufs); peak_vec.push(peak); }
            Err(_) => { lufs_vec.push(-70.0); peak_vec.push(-70.0); failures += 1; }
        }
    } else {
        // Multi-channel: use channelsplit + per-channel ebur128
        for ch in 0..channels {
            check(ctx)?;
            let pct = 0.05 + 0.20 * (ch as f64 / channels as f64);
            emit(ctx, "loudness", pct);
            match analyze_single_channel(app, feed, Some(ch), input_codec, ctx, pct).await {
                Ok((lufs, peak)) => { lufs_vec.push(lufs); peak_vec.push(peak); }
                Err(_) => { lufs_vec.push(-70.0); peak_vec.push(-70.0); failures += 1; }
            }
        }
    }

    if failures >= channels.max(1) {
        return Err("Could not decode this file for analysis".into());
    }

    Ok((lufs_vec, peak_vec, failures))
}

async fn analyze_single_channel(
    app: &AppHandle,
    feed: &Path,
    channel: Option<u32>,
    input_codec: &[String],
    ctx: Option<&ScanCtx>,
    pct: f64,
) -> Result<(f64, f64), String> {
    let feed_str = feed.to_string_lossy().to_string();

    // Build filter: optionally extract a single channel via pan, then run ebur128.
    // Using pan=mono instead of channelsplit avoids hardcoding a channel layout.
    // `-t` (input option, before `-i`) limits analysis to a representative
    // sample so a long recording can't make this pass run for minutes.
    let secs = crate::ffmpeg::ANALYSIS_SAMPLE_SECS.to_string();
    let mut args: Vec<String> = input_codec.to_vec();
    if let Some(ch) = channel {
        let pan = format!("pan=mono|c0=c{}", ch);
        let filter = format!("{},ebur128=peak=true", pan);
        args.extend([
            "-t".into(), secs,
            "-i".into(), feed_str,
            "-af".into(), filter,
            "-f".into(), "null".into(), "-".into(),
        ]);
    } else {
        args.extend([
            "-t".into(), secs,
            "-i".into(), feed_str,
            "-af".into(), "ebur128=peak=true".into(),
            "-f".into(), "null".into(), "-".into(),
        ]);
    }

    // Bounded timeout backstop — the -t cap means a healthy run finishes in
    // seconds, so a wedged ffmpeg can never hang the Scan. The heartbeat
    // keeps the frontend's stall watchdog fed while the 120s backstop drains.
    let output = sidecar_with_heartbeat(app, crate::helpers::ffmpeg_bin_name(), args, 120, ctx, "loudness", pct)
        .await
        .ok_or_else(|| "Loudness analysis timed out".to_string())?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Parse integrated loudness: "I: -XX.X LUFS"
    let lufs_re = Regex::new(r"I:\s+(-?\d+\.?\d*)\s+LUFS").unwrap();
    let lufs = lufs_re
        .captures_iter(&stderr)
        .last()
        .and_then(|c| c[1].parse::<f64>().ok());

    // Parse true peak: "Peak: -XX.X dBFS"
    let peak_re = Regex::new(r"Peak:\s+(-?\d+\.?\d*)\s+dBFS").unwrap();
    let peak = peak_re
        .captures_iter(&stderr)
        .last()
        .and_then(|c| c[1].parse::<f64>().ok());

    // A run that produced neither measurement AND exited non-zero never
    // decoded anything — report it instead of pretending silence.
    if lufs.is_none() && peak.is_none() && !output.success {
        return Err("FFmpeg could not decode this file".into());
    }

    Ok((lufs.unwrap_or(-70.0), peak.unwrap_or(-70.0)))
}

// ── Noise floor estimation ──────────────────────────────────────────────────

async fn estimate_noise_floor(app: &AppHandle, feed: &Path, input_codec: &[String], ctx: Option<&ScanCtx>) -> f64 {
    // Use astats' measured noise floor. The overall RMS level would include
    // speech and sits far above any sensible noise threshold, which made
    // denoising look "recommended" for virtually every normal recording.
    let mut args: Vec<String> = input_codec.to_vec();
    args.extend([
        "-t".into(), crate::ffmpeg::ANALYSIS_SAMPLE_SECS.to_string(),
        "-i".into(), feed.to_string_lossy().to_string(),
        "-af".into(), "astats=metadata=1".into(),
        "-f".into(), "null".into(), "-".into(),
    ]);

    if let Some(out) = sidecar_with_heartbeat(app, crate::helpers::ffmpeg_bin_name(), args, 120, ctx, "noise", 0.29).await {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let noise_re = Regex::new(r"Noise floor dB:\s+(-?\d+\.?\d*)").unwrap();
        // Use the last match: astats prints per-channel sections first,
        // then the Overall section.
        if let Some(cap) = noise_re.captures_iter(&stderr).last() {
            if let Ok(floor) = cap[1].parse::<f64>() {
                return floor;
            }
        }
    }

    -60.0 // Assume quiet if analysis fails (or floor is -inf, i.e. silence)
}

// ── Sample rate probing ─────────────────────────────────────────────────────

async fn probe_sample_rate(app: &AppHandle, feed: &Path) -> Option<u32> {
    let args: Vec<String> = vec![
        "-v".into(), "quiet".into(),
        "-print_format".into(), "json".into(),
        "-show_streams".into(),
        "-select_streams".into(), "a:0".into(),
        feed.to_string_lossy().to_string(),
    ];

    let output = crate::ffmpeg::sidecar_output_opt(app, ffprobe_bin_name(), args, 30).await?;

    let text = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["streams"][0]["sample_rate"]
        .as_str()?
        .parse::<u32>()
        .ok()
}

// ── Smart Turn detection ────────────────────────────────────────────────────
//
// Uses the Pipecat Smart Turn v3 ONNX model to detect speaker turn boundaries.
// Each 8-second window is converted to Whisper-style log-mel features
// (see mel.rs); the model's logit becomes a turn-completion probability.

async fn detect_turns(
    app: &AppHandle,
    feed: &Path,
    channels: u32,
    input_codec: &[String],
    ctx: Option<&ScanCtx>,
) -> Vec<TurnSegment> {
    // Try loading the Smart Turn model — if not available, return empty
    let model_path = match crate::models::model_path(app, "smart-turn-v3-int8.onnx") {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    // Phase 1 (async): decode each channel to a 16kHz mono temp WAV. TempFile
    // drop guards own cleanup on EVERY path — early returns, cancel/budget
    // breaks, and panics in the inference task below (an unwinding panic
    // would skip any explicit cleanup loop and leak up to 16 × ~6MB).
    let mut decoded: Vec<(u32, crate::safety::TempFile)> = Vec::new();
    for ch in 0..channels {
        if ctx.map(|c| c.cancelled()).unwrap_or(false) { break; }
        let pct = 0.45 + 0.10 * (ch as f64 / channels as f64);
        emit(ctx, "turns", pct);

        let tmp = crate::safety::TempFile::new(
            std::env::temp_dir().join(format!("depoaudio_turn_ch{}_{}.wav", ch, uuid::Uuid::new_v4())),
        );
        let pan_filter = if channels > 1 {
            format!("pan=mono|c0=c{},aresample=16000", ch)
        } else {
            "aresample=16000".into()
        };

        let mut args: Vec<String> = input_codec.to_vec();
        args.extend([
            "-t".into(), crate::ffmpeg::ANALYSIS_SAMPLE_SECS.to_string(),
            "-i".into(), feed.to_string_lossy().to_string(),
            "-af".into(), pan_filter,
            // -ac 1 matters on the channels==1 fallback path (probe failed):
            // without it a multichannel file would feed interleaved samples
            // to the model as if they were mono
            "-ac".into(), "1".into(),
            "-acodec".into(), "pcm_s16le".into(),
            "-y".into(), tmp.to_string_lossy().to_string(),
        ]);

        let ok = sidecar_with_heartbeat(app, crate::helpers::ffmpeg_bin_name(), args, 120, ctx, "turns", pct)
            .await
            .map(|o| o.success)
            .unwrap_or(false);

        if ok && tmp.exists() {
            decoded.push((ch, tmp));
        }
        // else: tmp drops here and removes any partial decode
    }

    if decoded.is_empty() { return Vec::new(); }

    // Phase 2 (blocking pool): mel features + ONNX inference. This is minutes
    // of CPU-bound work in the worst case; on the async runtime it pinned
    // worker threads until tokio timers across the whole app stopped firing.
    // Inside the loop: cancellation check + wall-clock budget, and progress
    // events so the scan is never silent for minutes.
    let ctx_owned = ctx.cloned();
    let result = tauri::async_runtime::spawn_blocking(move || {
        let mut all_turns: Vec<TurnSegment> = Vec::new();
        // On any early return, break, or panic below, the TempFile guards in
        // `decoded` (moved into this closure) delete the WAVs as they drop.
        let mut session = match crate::models::load_session(&model_path) {
            Ok(s) => s,
            Err(_) => return all_turns,
        };

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(TURNS_BUDGET_SECS);
        let total_channels = decoded.len().max(1) as f64;

        'channels: for (idx, (ch, tmp)) in decoded.into_iter().enumerate() {
            let samples: Vec<f32> = match hound::WavReader::open(&*tmp) {
                Ok(reader) => reader
                    .into_samples::<i16>()
                    .filter_map(|s| s.ok())
                    .map(|s| s as f32 / 32768.0)
                    .collect(),
                Err(_) => continue,
            };

            let sample_rate = 16000usize;
            let window_size = sample_rate * 8; // 8 seconds
            let stride = sample_rate; // 1 second stride
            let total_windows = (samples.len().saturating_sub(window_size) / stride + 1).max(1);
            let mut window_idx = 0usize;
            let mut pos = 0usize;
            let mut turn_start: Option<f64> = None;

            while pos + window_size <= samples.len() {
                // Budget + cancellation: bounded slowness must stay bounded,
                // and a cancelled scan must actually stop computing.
                if std::time::Instant::now() > deadline { break 'channels; }
                if let Some(c) = &ctx_owned {
                    if c.cancelled() { break 'channels; }
                    if window_idx % 16 == 0 {
                        let ch_frac = (idx as f64 + window_idx as f64 / total_windows as f64) / total_channels;
                        c.emit("turns", 0.55 + 0.30 * ch_frac);
                    }
                }

                let window = &samples[pos..pos + window_size];

                // Smart Turn v3 takes Whisper-style log-mel features
                // [1, 80, 800] under "input_features" and emits a raw logit
                // under "logits" (sigmoid -> turn-completion probability)
                let feats = crate::mel::log_mel_8s(window);
                let input = ndarray::Array3::from_shape_vec(
                    (1, crate::mel::N_MELS, crate::mel::N_FRAMES),
                    feats,
                ).ok();

                let prob = if let Some(input_arr) = input {
                    match ort::value::Tensor::from_array(input_arr) {
                        Ok(tensor) => {
                            match session.run(ort::inputs!["input_features" => tensor]) {
                                Ok(outputs) => {
                                    outputs.get("logits")
                                        .and_then(|v| v.try_extract_tensor::<f32>().ok())
                                        .and_then(|t| t.1.first().copied())
                                        .map(|logit| 1.0 / (1.0 + (-logit).exp()))
                                        .unwrap_or(0.0)
                                }
                                Err(_) => 0.0,
                            }
                        }
                        Err(_) => 0.0,
                    }
                } else {
                    0.0
                };

                let time = pos as f64 / sample_rate as f64;

                // Turn-end detected (probability > 0.5)
                if prob > 0.5 {
                    if turn_start.is_none() {
                        // Mark the beginning of the current speaking segment
                        // (look back from the turn-end to find approximate start)
                        turn_start = Some((time - 4.0).max(0.0));
                    }
                    // Close the turn segment
                    if let Some(start) = turn_start.take() {
                        all_turns.push(TurnSegment {
                            start,
                            end: time + 4.0, // end of the 8-second window
                            channel: ch,
                            confidence: prob as f64,
                        });
                    }
                }

                pos += stride;
                window_idx += 1;
            }
            // tmp (TempFile) drops here — WAV deleted; a budget/cancel break
            // or panic drops the remaining iterator, deleting the rest
        }

        all_turns
    })
    .await;

    let mut all_turns = result.unwrap_or_default();

    // Merge adjacent turns on same channel within 1 second
    all_turns.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
    let mut merged = Vec::new();
    for turn in all_turns {
        if let Some(last) = merged.last_mut() {
            let last_turn: &mut TurnSegment = last;
            if last_turn.channel == turn.channel && (turn.start - last_turn.end).abs() < 1.0 {
                last_turn.end = turn.end;
                last_turn.confidence = last_turn.confidence.max(turn.confidence);
                continue;
            }
        }
        merged.push(turn);
    }

    merged
}
