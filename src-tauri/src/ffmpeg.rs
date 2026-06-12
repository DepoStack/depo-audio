use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

use crate::helpers::{ffmpeg_bin_name, ffprobe_bin_name};
use crate::types::{ConvertJob, ProgressEvent};

fn time_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"time=(\d+):(\d+):(\d+\.\d+)").unwrap())
}

// ── Probe helpers ─────────────────────────────────────────────────────────────

// Note: ffprobe auto-detects codecs and does not accept ffmpeg input options
// like -acodec, so the probe helpers take no input codec arguments.
pub(crate) async fn probe_duration(app: &AppHandle, feed: &Path) -> Option<f64> {
    let args: Vec<String> = vec![
        "-v".into(), "quiet".into(),
        "-print_format".into(), "json".into(),
        "-show_format".into(),
        feed.to_string_lossy().to_string(),
    ];
    let output = app.shell().sidecar(ffprobe_bin_name()).ok()?
        .args(args).output().await.ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["format"]["duration"].as_str()?.parse::<f64>().ok()
}

/// Probe the channel count of the first audio stream. Returns None when the
/// probe fails — callers that build per-channel filtergraphs must not guess:
/// a wrong count silently produces silent output channels.
pub(crate) async fn probe_channels(app: &AppHandle, feed: &Path) -> Option<u32> {
    let args: Vec<String> = vec![
        "-v".into(), "quiet".into(),
        "-print_format".into(), "json".into(),
        "-show_streams".into(),
        "-select_streams".into(), "a:0".into(),
        feed.to_string_lossy().to_string(),
    ];
    let cmd = app.shell().sidecar(ffprobe_bin_name()).ok()?;
    let out = cmd.args(args).output().await.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let ch = v["streams"][0]["channels"].as_u64()?;
    if ch > 0 { Some(ch as u32) } else { None }
}

// ── Filter chain builder ─────────────────────────────────────────────────────

/// Build the processing filter chain, optionally injecting a computed gain
/// value from auto-leveling analysis.
pub(crate) async fn build_proc_filters_with_gain(
    app: &AppHandle,
    opts: &ConvertJob,
    feed: &Path,
    auto_gain: Option<f64>,
) -> Vec<String> {
    let mut filters = Vec::new();

    // De-clipping runs first — reconstruct clipped peaks before other processing
    if opts.declip    { filters.push("adeclip=w=55:o=50".into()); }

    // High-pass filter removes low-frequency noise (HVAC, handling, rumble)
    if opts.hpf       { filters.push(format!("highpass=f={}", opts.hpf_cutoff as u32)); }

    // Auto-level gain injection (from analysis-computed per-channel gain)
    if let Some(gain) = auto_gain {
        if (gain - 1.0).abs() > 0.01 {
            filters.push(format!("volume={:.4}", gain));
        }
    }

    // Loudness normalization for consistent output level
    if opts.normalize { filters.push(format!("loudnorm=I={}:TP={}:LRA=11", opts.normalize_lufs, opts.normalize_tp)); }

    // Silence trimming removes dead air at start/end
    if opts.trim      { filters.push(format!("silenceremove=start_periods=1:start_duration=0.3:start_threshold={}dB:stop_periods=-1:stop_duration=0.3:stop_threshold={}dB", opts.silence_thresh, opts.silence_thresh)); }

    // Fade in/out for smooth start and end
    if opts.fade {
        let dur = probe_duration(app, feed).await;
        filters.push(format!("afade=t=in:d={}", opts.fade_dur));
        if let Some(d) = dur {
            let start = (d - opts.fade_dur).max(0.0);
            filters.push(format!("afade=t=out:st={start:.3}:d={}", opts.fade_dur));
        }
    }
    filters
}

// ── Run FFmpeg sidecar ────────────────────────────────────────────────────────

/// Minimum allowed FFmpeg timeout, guarding against bogus persisted settings.
const MIN_FFMPEG_TIMEOUT_SECS: u64 = 30;

pub(crate) async fn run_ffmpeg_with_timeout(app: &AppHandle, args: Vec<String>, job_id: &str, timeout_secs: u64) -> Result<(), String> {
    let (mut rx, child) = app
        .shell()
        .sidecar(ffmpeg_bin_name())
        .map_err(|e| e.to_string())?
        .args(args)
        .spawn()
        .map_err(|e| e.to_string())?;

    let mut stderr_acc = String::new();
    let time_re = time_regex();
    let timeout_secs = timeout_secs.max(MIN_FFMPEG_TIMEOUT_SECS);
    let started = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    loop {
        // Bound the wait so a silently wedged FFmpeg is still killed
        let remaining = match timeout.checked_sub(started.elapsed()) {
            Some(r) => r,
            None => {
                let _ = child.kill();
                return Err(format!("FFmpeg process timed out after {timeout_secs} seconds and was killed"));
            }
        };
        let event = match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                let _ = child.kill();
                return Err(format!("FFmpeg process timed out after {timeout_secs} seconds and was killed"));
            }
        };

        match event {
            CommandEvent::Stderr(bytes) => {
                let line = String::from_utf8_lossy(&bytes).to_string();
                stderr_acc.push_str(&line);
                if let Some(cap) = time_re.captures(&line) {
                    let h: f64 = cap[1].parse().unwrap_or(0.0);
                    let m: f64 = cap[2].parse().unwrap_or(0.0);
                    let s: f64 = cap[3].parse().unwrap_or(0.0);
                    let secs = h * 3600.0 + m * 60.0 + s;
                    let _ = app.emit("convert:progress", ProgressEvent { id: job_id.to_string(), seconds: secs, phase: None });
                }
            }
            CommandEvent::Terminated(status) => {
                if status.code != Some(0) {
                    let lines: Vec<&str> = stderr_acc.lines()
                        .filter(|l| !l.starts_with("ffmpeg version") && !l.starts_with("built") && !l.starts_with("lib") && !l.starts_with("configuration:"))
                        .collect();
                    let msg = lines.iter().rev().take(4).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
                    // Sanitize: strip file paths from error messages
                    let sanitized = msg.replace(|c: char| c == '/' || c == '\\', "_")
                        .chars().take(300).collect::<String>();
                    return Err(sanitized);
                }
                return Ok(());
            }
            _ => {}
        }
    }
    Ok(())
}
