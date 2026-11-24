use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use regex::Regex;
use tauri::{AppHandle, Emitter};

use crate::ffmpeg::{probe_channels_cancellable, probe_duration_cancellable};
use crate::helpers::{ffprobe_bin_name, AudioFeed};
use crate::types::AnalysisResult;

// ── Audio analysis engine ────────────────────────────────────────────────────
//
// Pre-scans audio with FFmpeg/ffprobe to report objective signal and container
// observations available without learned-model inference.

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

/// Bounded conversion-time auto-level pass on an already prepared feed (the
/// conversion's own, so an FTR recording is not read or split twice). This
/// intentionally performs only the representative loudness measurements
/// needed for channel gains; it does not invoke VAD, Smart Turn, quality
/// scoring, speaker models, or full PCM predecode.
pub(crate) async fn analyze_feed_channel_gains(
    app: &AppHandle,
    feed: &AudioFeed,
    ctx: Option<&ScanCtx>,
) -> Result<Vec<f64>, String> {
    check(ctx)?;
    let is_cancelled = || ctx.map(|context| context.cancelled()).unwrap_or(false);
    let channels = probe_channels_cancellable(app, feed, Some(&is_cancelled)).await;
    check(ctx)?;
    let channels = channels.ok_or("Cannot auto-level: unable to determine the input's channel count")?;
    if channels == 0 || channels > MAX_SCAN_CHANNELS {
        return Err(format!(
            "Cannot auto-level a recording with {channels} channels; the supported maximum is {MAX_SCAN_CHANNELS}."
        ));
    }

    let (per_channel_lufs, _, _) = analyze_loudness_and_peaks(app, feed, channels, ctx).await?;
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

/// `prepare_audio_feed_cancellable` with a progress heartbeat. Preparing a
/// `.dcr` reads the whole recording, writes a decoded copy and may run an
/// FFmpeg merge, which on a long recording can outlast the frontend's stall
/// watchdog; the heartbeat says the scan is alive while that runs.
async fn prepare_with_heartbeat(
    app: &AppHandle,
    source: &Path,
    cancelled: Arc<dyn Fn() -> bool + Send + Sync>,
    ctx: Option<&ScanCtx>,
) -> Result<AudioFeed, String> {
    let prepare = crate::helpers::prepare_audio_feed_cancellable(app, source.to_path_buf(), cancelled);
    tokio::pin!(prepare);
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(HEARTBEAT_SECS), &mut prepare).await {
            Ok(prepared) => return prepared,
            Err(_) => emit(ctx, "prepare", 0.01),
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
    // An FTR recording is read through here and either decoded by FFmpeg's
    // native decoder or split into per-channel files first (crate::trm).
    let prepared = prepare_with_heartbeat(app, source, preparation_cancelled.clone(), ctx).await;
    check(ctx)?;
    let mut prepared_feed = prepared?;
    let feed = &prepared_feed;

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

    // Run loudness + peak analysis per channel. If FFmpeg's native FTR
    // decoder read none of the channels, split the channels and try once more.
    let loudness = analyze_loudness_and_peaks(app, feed, channels, ctx).await;
    check(ctx)?;
    let (per_channel_lufs, per_channel_peak, loudness_failures) = match loudness {
        Err(error) if feed.used_native_ftr() => {
            let retry = crate::helpers::prepare_audio_feed_after_native_failure(
                app,
                source.to_path_buf(),
                preparation_cancelled,
            )
            .await;
            check(ctx)?;
            prepared_feed =
                retry.map_err(|retry_error| format!("{error}; splitting its channels also failed: {retry_error}"))?;
            analyze_loudness_and_peaks(app, &prepared_feed, channels, ctx).await?
        }
        other => other?,
    };
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

    // DepoAudio intentionally performs no learned-model inference. Retain the
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
    feed: &AudioFeed,
    channels: u32,
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
        match analyze_single_channel(app, feed, None, ctx, 0.05).await {
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
            match analyze_single_channel(app, feed, Some(ch), ctx, pct).await {
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

/// FFmpeg arguments for one loudness/peak pass after the input prelude
/// (the inputs come from `feed_loudness_pass_args`): a bounded sample (`-t`
/// before `-i`), the converted audio stream mapped explicitly (`-map 0:a:0`,
/// the stream the channel probe counted), optionally one channel isolated
/// with `pan=mono`, then `ebur128`. Using pan=mono instead of channelsplit
/// avoids hardcoding a channel layout.
#[cfg(test)]
pub(crate) fn loudness_pass_args(feed: &str, channel: Option<u32>, secs: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["-t".into(), secs.into(), "-i".into(), feed.into()];
    args.extend(loudness_pass_tail(channel, crate::helpers::source_audio_map_args()));
    args
}

fn loudness_pass_tail(channel: Option<u32>, map: Vec<String>) -> Vec<String> {
    let filter = match channel {
        Some(ch) => format!("pan=mono|c0=c{ch},ebur128=peak=true"),
        None => "ebur128=peak=true".into(),
    };
    let mut args = map;
    args.extend(["-af".into(), filter, "-f".into(), "null".into(), "-".into()]);
    args
}

/// One loudness pass for `feed`. A feed with one input per channel (a split
/// FTR recording) reads only that channel's file, so a 16-channel scan does
/// not decode all 16 channels 16 times.
fn feed_loudness_pass_args(feed: &AudioFeed, channel: Option<u32>, secs: &str) -> Vec<String> {
    let per_input = vec!["-t".to_string(), secs.to_string()];
    if let Some((input, inner)) = channel.and_then(|channel| feed.channel_input_args(channel, &per_input)) {
        let mut args = input;
        args.extend(loudness_pass_tail(inner, crate::helpers::source_audio_map_args()));
        return args;
    }
    if feed.is_merged() {
        let filter = match channel {
            Some(ch) => format!("pan=mono|c0=c{ch},ebur128=peak=true"),
            None => "ebur128=peak=true".into(),
        };
        let mut args = feed.input_args(&per_input);
        args.extend(feed.source_output_args(Some(&filter)));
        args.extend(["-f".into(), "null".into(), "-".into()]);
        return args;
    }
    let mut args = feed.input_args(&per_input);
    args.extend(loudness_pass_tail(channel, crate::helpers::source_audio_map_args()));
    args
}

async fn analyze_single_channel(
    app: &AppHandle,
    feed: &AudioFeed,
    channel: Option<u32>,
    ctx: Option<&ScanCtx>,
    pct: f64,
) -> Result<(f64, f64), String> {
    // `-t` (input option, before `-i`) limits analysis to a representative
    // sample so a long recording can't make this pass run for minutes.
    let secs = crate::ffmpeg::ANALYSIS_SAMPLE_SECS.to_string();
    let mut args = crate::helpers::safe_ffmpeg_input_prelude();
    args.extend(feed_loudness_pass_args(feed, channel, &secs));

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

async fn probe_sample_rate(app: &AppHandle, feed: &AudioFeed, ctx: Option<&ScanCtx>) -> Option<u32> {
    if let Some(rate) = feed.known_sample_rate() {
        return Some(rate);
    }
    let feed = feed.path();
    let mut args: Vec<String> = vec![
        "-v".into(),
        "quiet".into(),
        "-print_format".into(),
        "json".into(),
        "-show_streams".into(),
        "-select_streams".into(),
        crate::helpers::source_audio_stream_specifier(),
    ];
    args.extend(crate::helpers::feed_format_args(feed));
    args.push(feed.to_string_lossy().to_string());

    let is_cancelled = || ctx.map(|context| context.cancelled()).unwrap_or(false);
    let output =
        crate::ffmpeg::sidecar_output_cancellable(app, ffprobe_bin_name(), args, 30, Some(&is_cancelled)).await?;

    let text = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["streams"][0]["sample_rate"].as_str()?.parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::{build_recommendations, channel_gains_from_lufs, loudness_pass_args};

    #[test]
    fn loudness_passes_map_the_converted_audio_stream() {
        assert_eq!(
            loudness_pass_args("/in.mkv", Some(2), "180"),
            vec![
                "-t",
                "180",
                "-i",
                "/in.mkv",
                "-map",
                "0:a:0",
                "-af",
                "pan=mono|c0=c2,ebur128=peak=true",
                "-f",
                "null",
                "-"
            ]
        );
        assert_eq!(
            loudness_pass_args("/in.mkv", None, "180"),
            vec![
                "-t",
                "180",
                "-i",
                "/in.mkv",
                "-map",
                "0:a:0",
                "-af",
                "ebur128=peak=true",
                "-f",
                "null",
                "-"
            ]
        );
    }

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
