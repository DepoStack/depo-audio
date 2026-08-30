use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use regex::Regex;
use tauri::{AppHandle, Emitter};

use crate::ffmpeg::{ensure_ftr_decoder, probe_channels_cancellable, probe_duration_cancellable};
use crate::helpers::ffprobe_bin_name;
use crate::types::AnalysisResult;

// ── Audio analysis engine ────────────────────────────────────────────────────
//
// Pre-scans audio with FFmpeg/ffprobe to report objective signal and container
// observations available in the v1.0.3 release.

/// Target loudness for auto-leveling (LUFS).
const TARGET_LUFS: f64 = -16.0;
/// Channels quieter than this are considered silent (LUFS).
const SILENCE_THRESHOLD: f64 = -60.0;
/// LUFS spread across channels that triggers auto-leveling recommendation.
const LEVELING_THRESHOLD: f64 = 3.0;
/// Peak dBFS threshold above which clipping is detected.
const CLIPPING_THRESHOLD: f64 = -0.5;
/// Sample rate at or below which the source is reported as narrow-band.
const NARROWBAND_RATE: u32 = 16000;
/// Court recorders top out at 16 channels; anything larger is a corrupt
/// header and must not become a loop bound.
const MAX_SCAN_CHANNELS: u32 = 16;

/// Context for a user-visible Scan: progress events + cancellation.
/// Conversion-time analysis (auto-level) passes None and runs silently.
#[derive(Clone)]
pub(crate) struct ScanCtx {
    pub app: AppHandle,
    pub path: String,
    epoch: Arc<AtomicU64>,
    my_gen: u64,
    emit_progress: bool,
}

impl ScanCtx {
    pub fn new(app: AppHandle, path: String, epoch: Arc<AtomicU64>) -> Self {
        let my_gen = epoch.load(Ordering::SeqCst);
        Self {
            app,
            path,
            epoch,
            my_gen,
            emit_progress: true,
        }
    }

    /// Reuse the analysis engine for conversion-time auto-leveling while
    /// binding its sidecars and cancellation checkpoints to the conversion batch
    /// cancellation token. Conversion analysis must not leak scan-progress
    /// events into the Scan UI.
    pub fn silent(app: AppHandle, path: String, epoch: Arc<AtomicU64>, my_gen: u64) -> Self {
        Self {
            app,
            path,
            epoch,
            my_gen,
            emit_progress: false,
        }
    }

    /// True once cancel_scan_cmd has bumped the epoch past this scan's start.
    pub fn cancelled(&self) -> bool {
        self.epoch.load(Ordering::Relaxed) != self.my_gen
    }

    pub fn check(&self) -> Result<(), String> {
        if self.cancelled() {
            Err("Scan cancelled".into())
        } else {
            Ok(())
        }
    }

    /// Within-file progress: phase name + estimated fraction complete [0, 1].
    /// `gen` lets the frontend drop trailing events from a cancelled scan
    /// that would otherwise pollute a successor scan of the same file.
    pub fn emit(&self, phase: &str, pct: f64) {
        if !self.emit_progress {
            return;
        }
        let _ = self.app.emit(
            "scan:progress",
            serde_json::json!({ "path": self.path, "phase": phase, "pct": pct, "gen": self.my_gen }),
        );
    }
}

fn emit(ctx: Option<&ScanCtx>, phase: &str, pct: f64) {
    if let Some(c) = ctx {
        c.emit(phase, pct);
    }
}

fn check(ctx: Option<&ScanCtx>) -> Result<(), String> {
    match ctx {
        Some(c) => c.check(),
        None => Ok(()),
    }
}

fn channel_gains_from_lufs(per_channel_lufs: &[f64]) -> Vec<f64> {
    per_channel_lufs
        .iter()
        .map(|&lufs| {
            if lufs <= SILENCE_THRESHOLD {
                1.0
            } else {
                let gain = 10_f64.powf((TARGET_LUFS - lufs) / 20.0);
                gain.clamp(0.1, 10.0)
            }
        })
        .collect()
}

/// Bounded conversion-time auto-level pass. This intentionally performs only
/// the representative loudness measurements needed for channel gains; it does
/// not invoke VAD, Smart Turn, quality scoring, speaker models, or full PCM
/// predecode.
pub(crate) async fn analyze_channel_gains(
    app: &AppHandle,
    path: &str,
    max_size: u64,
    ctx: Option<&ScanCtx>,
) -> Result<Vec<f64>, String> {
    let source = Path::new(path);
    crate::safety::check_file_safe_with_limit(source, max_size)?;
    let preparation_context = ctx.cloned();
    let preparation_cancelled: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || preparation_context.as_ref().is_some_and(ScanCtx::cancelled));
    let prepared = crate::helpers::prepare_audio_feed_cancellable(source.to_path_buf(), preparation_cancelled).await;
    check(ctx)?;
    let (feed_path, _feed_guard) = prepared?;
    let feed = feed_path.as_path();

    let input_codec = crate::helpers::input_codec_args(feed);
    if !input_codec.is_empty() {
        ensure_ftr_decoder(app).await?;
        check(ctx)?;
    }

    let is_cancelled = || ctx.map(|context| context.cancelled()).unwrap_or(false);
    let channels = probe_channels_cancellable(app, feed, Some(&is_cancelled)).await;
    check(ctx)?;
    let channels = channels.ok_or("Cannot auto-level: unable to determine the input's channel count")?;
    if channels == 0 || channels > MAX_SCAN_CHANNELS {
        return Err(format!(
            "Cannot auto-level a recording with {channels} channels; the supported maximum is {MAX_SCAN_CHANNELS}."
        ));
    }

    let (per_channel_lufs, _, _) = analyze_loudness_and_peaks(app, feed, channels, &input_codec, ctx).await?;
    check(ctx)?;
    Ok(channel_gains_from_lufs(&per_channel_lufs))
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
    let source = Path::new(path);
    crate::safety::check_file_safe(source)?;
    let preparation_context = ctx.cloned();
    let preparation_cancelled: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || preparation_context.as_ref().is_some_and(ScanCtx::cancelled));
    let prepared = crate::helpers::prepare_audio_feed_cancellable(source.to_path_buf(), preparation_cancelled).await;
    check(ctx)?;
    let (feed_path, _feed_guard) = prepared?;
    let feed = feed_path.as_path();

    // Select FFmpeg's native FTR decoder explicitly. Plain AAC cannot decode
    // FTR's modified, per-channel packet layout.
    let input_codec = crate::helpers::input_codec_args(feed);
    if !input_codec.is_empty() {
        ensure_ftr_decoder(app).await?;
    }

    emit(ctx, "probe", 0.02);
    // Probe basic metadata. On probe failure assume ONE channel — inventing
    // phantom channels multiplies every per-channel pass (each with a long
    // timeout backstop) on exactly the files that are already struggling.
    // Cap the count so a corrupt header can't become a loop bound.
    // Emit between the chained probes: three wedged 30s probes back-to-back
    // would otherwise exceed the frontend's stall watchdog.
    let is_cancelled = || ctx.map(|context| context.cancelled()).unwrap_or(false);
    let channels = probe_channels_cancellable(app, feed, Some(&is_cancelled))
        .await
        .unwrap_or(1)
        .min(MAX_SCAN_CHANNELS);
    check(ctx)?;
    emit(ctx, "probe", 0.03);
    let duration = probe_duration_cancellable(app, feed, Some(&is_cancelled))
        .await
        .unwrap_or(0.0);
    check(ctx)?;
    emit(ctx, "probe", 0.04);
    let sample_rate = probe_sample_rate(app, feed, ctx).await.unwrap_or(48000);
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

    // v1.0.3 intentionally performs no learned-model inference. Retain the
    // wire fields for compatibility, but report no neural analysis result.
    let needs_denoise = false;
    let is_narrowband = sample_rate <= NARROWBAND_RATE;
    let channel_gains = channel_gains_from_lufs(&per_channel_lufs);
    let recommendations = build_recommendations(
        needs_leveling,
        &active_lufs,
        has_clipping,
        &per_channel_peak,
        is_narrowband,
        sample_rate,
        loudness_failures,
    );

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
        turns: Vec::new(),
        channel_gains,
        recommendations,
        quality_score: None,
        speaker_count: None,
        speech_ratio: None,
    })
}

fn build_recommendations(
    needs_leveling: bool,
    active_lufs: &[f64],
    has_clipping: bool,
    per_channel_peak: &[f64],
    is_narrowband: bool,
    sample_rate: u32,
    loudness_failures: u32,
) -> Vec<String> {
    let mut recommendations = Vec::new();

    if needs_leveling && active_lufs.len() > 1 {
        let spread = active_lufs.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            - active_lufs.iter().copied().fold(f64::INFINITY, f64::min);
        recommendations.push(format!(
            "{spread:.1} dB spread across channels — auto-leveling recommended"
        ));
    }

    if has_clipping {
        let clipped: Vec<usize> = per_channel_peak
            .iter()
            .enumerate()
            .filter(|(_, peak)| **peak >= CLIPPING_THRESHOLD)
            .map(|(index, _)| index + 1)
            .collect();
        recommendations.push(format!(
            "Clipping detected on channel{} {} — de-clipping recommended",
            if clipped.len() > 1 { "s" } else { "" },
            clipped.iter().map(usize::to_string).collect::<Vec<_>>().join(", ")
        ));
    }

    if is_narrowband {
        recommendations.push(format!(
            "Narrow-band source detected ({sample_rate} Hz) — high-frequency detail may be absent from the recording"
        ));
    }

    if loudness_failures > 0 {
        recommendations.push(
            "Some channels could not be decoded for analysis — convert the file first, then scan the converted output"
                .into(),
        );
    }

    recommendations
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
            Ok((lufs, peak)) => {
                lufs_vec.push(lufs);
                peak_vec.push(peak);
            }
            Err(_) => {
                lufs_vec.push(-70.0);
                peak_vec.push(-70.0);
                failures += 1;
            }
        }
    } else {
        // Multi-channel: use channelsplit + per-channel ebur128
        for ch in 0..channels {
            check(ctx)?;
            let pct = 0.05 + 0.20 * (ch as f64 / channels as f64);
            emit(ctx, "loudness", pct);
            match analyze_single_channel(app, feed, Some(ch), input_codec, ctx, pct).await {
                Ok((lufs, peak)) => {
                    lufs_vec.push(lufs);
                    peak_vec.push(peak);
                }
                Err(_) => {
                    lufs_vec.push(-70.0);
                    peak_vec.push(-70.0);
                    failures += 1;
                }
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
    let mut args = crate::helpers::safe_ffmpeg_input_prelude();
    args.extend(input_codec.iter().cloned());
    if let Some(ch) = channel {
        let pan = format!("pan=mono|c0=c{}", ch);
        let filter = format!("{},ebur128=peak=true", pan);
        args.extend([
            "-t".into(),
            secs,
            "-i".into(),
            feed_str,
            "-af".into(),
            filter,
            "-f".into(),
            "null".into(),
            "-".into(),
        ]);
    } else {
        args.extend([
            "-t".into(),
            secs,
            "-i".into(),
            feed_str,
            "-af".into(),
            "ebur128=peak=true".into(),
            "-f".into(),
            "null".into(),
            "-".into(),
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

// ── Sample rate probing ─────────────────────────────────────────────────────

async fn probe_sample_rate(app: &AppHandle, feed: &Path, ctx: Option<&ScanCtx>) -> Option<u32> {
    let args: Vec<String> = vec![
        "-v".into(),
        "quiet".into(),
        "-print_format".into(),
        "json".into(),
        "-show_streams".into(),
        "-select_streams".into(),
        "a:0".into(),
        feed.to_string_lossy().to_string(),
    ];

    let is_cancelled = || ctx.map(|context| context.cancelled()).unwrap_or(false);
    let output =
        crate::ffmpeg::sidecar_output_cancellable(app, ffprobe_bin_name(), args, 30, Some(&is_cancelled)).await?;

    let text = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["streams"][0]["sample_rate"].as_str()?.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::{build_recommendations, channel_gains_from_lufs};

    #[test]
    fn channel_gain_analysis_leaves_silence_and_bounds_active_gain() {
        let gains = channel_gains_from_lufs(&[-70.0, -16.0, -36.0, 20.0]);
        assert_eq!(gains[0], 1.0);
        assert!((gains[1] - 1.0).abs() < 0.0001);
        assert_eq!(gains[2], 10.0);
        assert_eq!(gains[3], 0.1);
    }

    #[test]
    fn released_recommendations_never_offer_learned_cleanup() {
        let recommendations = build_recommendations(true, &[-24.0, -14.0], true, &[-0.2, -8.0], true, 16_000, 1);
        let visible = recommendations.join(" ").to_ascii_lowercase();

        assert!(visible.contains("auto-leveling"));
        assert!(visible.contains("de-clipping"));
        assert!(visible.contains("narrow-band"));
        for unavailable in ["denois", "enhance", "speaker", "vad", "dnsmos", "smart turn"] {
            assert!(
                !visible.contains(unavailable),
                "released analysis recommended unavailable capability: {unavailable}"
            );
        }
    }
}
