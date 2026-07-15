use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

use crate::helpers::{ffmpeg_bin_name, ffprobe_bin_name};
use crate::types::{ConvertJob, ProgressEvent};

/// How many seconds of audio the pre-scan / auto-level analysis reads.
/// Analysis is a heuristic recommendation, so a representative sample is
/// enough — and it bounds the work to ~constant time so the Scan (and the
/// auto-level pass during conversion) can't hang on a multi-hour recording.
pub(crate) const ANALYSIS_SAMPLE_SECS: u32 = 180;

fn time_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"time=(\d+):(\d+):(\d+\.\d+)").unwrap())
}

// ── Probe helpers ─────────────────────────────────────────────────────────────

/// Collected output of a completed sidecar run.
pub(crate) struct SidecarOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Run a sidecar (ffprobe/ffmpeg) to completion with a timeout. Returns None on
/// spawn error, failure, or timeout, so callers fall back to safe defaults.
/// Without this a wedged probe would hang `analyze_audio` — and therefore the
/// Scan button — forever. On timeout the child is KILLED, not abandoned:
/// orphaned decoders otherwise pile up at full CPU during a multi-file scan
/// and drag every later pass into its own timeout.
pub(crate) async fn sidecar_output_opt(
    app: &AppHandle,
    bin: &str,
    args: Vec<String>,
    secs: u64,
) -> Option<SidecarOutput> {
    let (mut rx, child) = app.shell().sidecar(bin).ok()?.args(args).spawn().ok()?;
    let mut child = Some(child);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut out = SidecarOutput { success: false, stdout: Vec::new(), stderr: Vec::new() };
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            if let Some(c) = child.take() { let _ = c.kill(); }
            return None;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Err(_) => {
                if let Some(c) = child.take() { let _ = c.kill(); }
                return None;
            }
            Ok(None) => break, // event channel closed: process is gone
            Ok(Some(CommandEvent::Stdout(bytes))) => out.stdout.extend_from_slice(&bytes),
            Ok(Some(CommandEvent::Stderr(bytes))) => out.stderr.extend_from_slice(&bytes),
            Ok(Some(CommandEvent::Terminated(status))) => out.success = status.code == Some(0),
            Ok(Some(_)) => {}
        }
    }
    Some(out)
}

// Note: ffprobe auto-detects codecs and does not accept ffmpeg input options
// like -acodec, so the probe helpers take no input codec arguments.
pub(crate) async fn probe_duration(app: &AppHandle, feed: &Path) -> Option<f64> {
    let args: Vec<String> = vec![
        "-v".into(), "quiet".into(),
        "-print_format".into(), "json".into(),
        "-show_format".into(),
        feed.to_string_lossy().to_string(),
    ];
    let output = sidecar_output_opt(app, ffprobe_bin_name(), args, 30).await?;
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
    let out = sidecar_output_opt(app, ffprobe_bin_name(), args, 30).await?;
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
    // Duration is only needed to place the fade-out, so only probe then.
    let duration = if opts.fade { probe_duration(app, feed).await } else { None };
    proc_filters(opts, auto_gain, duration)
}

/// Pure core of the filter-chain builder: everything except the duration
/// probe. Filter order is part of the output contract (de-clip → HPF → gain →
/// loudnorm → trim → fades) — see PARITY.md.
pub(crate) fn proc_filters(
    opts: &ConvertJob,
    auto_gain: Option<f64>,
    duration: Option<f64>,
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
        filters.push(format!("afade=t=in:d={}", opts.fade_dur));
        if let Some(d) = duration {
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

    // Some(Some(0)) = clean exit; Some(_) = non-zero/signal kill; None = the
    // event stream closed without a Terminated event ever arriving.
    let mut exit_status: Option<Option<i32>> = None;

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
                exit_status = Some(status.code);
                break;
            }
            _ => {}
        }
    }

    match exit_status {
        Some(Some(0)) => Ok(()),
        // Non-zero exit OR a signal kill (code == None): surface the tail of
        // stderr so the failure isn't reported as success.
        Some(_) => {
            let lines: Vec<&str> = stderr_acc.lines()
                .filter(|l| !l.starts_with("ffmpeg version") && !l.starts_with("built") && !l.starts_with("lib") && !l.starts_with("configuration:"))
                .collect();
            let msg = lines.iter().rev().take(4).cloned().collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" | ");
            // Sanitize: strip file paths from error messages
            let sanitized = msg.replace(['/', '\\'], "_")
                .chars().take(300).collect::<String>();
            if sanitized.trim().is_empty() {
                Err("FFmpeg exited abnormally (no error output)".into())
            } else {
                Err(sanitized)
            }
        }
        // The process channel closed without a Terminated event — treat as a
        // failure rather than silently reporting success on a possibly partial
        // or missing output file.
        None => Err("FFmpeg exited without reporting a status".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a ConvertJob from the frontend's wire format (camelCase JSON).
    /// Going through serde here also locks the IPC field names and defaults.
    fn job(extra: &str) -> ConvertJob {
        let mut base = serde_json::json!({
            "id": "t", "srcPath": "/in.wav", "outDir": "", "mode": "stereo",
            "format": "wav", "rate": "48000", "labels": [], "chanVols": [],
            "normalize": false, "trim": false, "fade": false, "fadeDur": 0.5,
            "hpf": false, "caseName": null,
        });
        // `extra` is a JSON fragment of overrides, e.g. `, "trim": true`
        let frag = extra.trim().trim_start_matches(',');
        let overrides: serde_json::Value =
            serde_json::from_str(&format!("{{{}}}", frag)).expect("valid override JSON");
        if let (Some(b), serde_json::Value::Object(o)) = (base.as_object_mut(), overrides) {
            for (k, v) in o { b.insert(k, v); }
        }
        serde_json::from_value(base).expect("valid job JSON")
    }

    #[test]
    fn all_processing_off_yields_empty_chain() {
        assert!(proc_filters(&job(""), None, None).is_empty());
    }

    #[test]
    fn declip_filter_is_stable() {
        assert_eq!(proc_filters(&job(r#", "declip": true"#), None, None),
                   vec!["adeclip=w=55:o=50"]);
    }

    #[test]
    fn hpf_uses_default_cutoff_80() {
        assert_eq!(proc_filters(&job(r#", "hpf": true"#), None, None),
                   vec!["highpass=f=80"]);
    }

    #[test]
    fn hpf_honors_custom_cutoff() {
        assert_eq!(proc_filters(&job(r#", "hpf": true, "hpfCutoff": 120.0"#), None, None),
                   vec!["highpass=f=120"]);
    }

    #[test]
    fn normalize_uses_default_lufs_and_tp() {
        assert_eq!(proc_filters(&job(r#", "normalize": true"#), None, None),
                   vec!["loudnorm=I=-16:TP=-1.5:LRA=11"]);
    }

    #[test]
    fn trim_uses_default_silence_threshold() {
        assert_eq!(proc_filters(&job(r#", "trim": true"#), None, None),
                   vec!["silenceremove=start_periods=1:start_duration=0.3:start_threshold=-50dB:stop_periods=-1:stop_duration=0.3:stop_threshold=-50dB"]);
    }

    #[test]
    fn near_unity_auto_gain_is_skipped() {
        // Gains within ±0.01 of 1.0 are noise, not leveling — omitted entirely
        assert!(proc_filters(&job(""), Some(1.005), None).is_empty());
        assert!(proc_filters(&job(""), Some(0.995), None).is_empty());
    }

    #[test]
    fn auto_gain_is_injected_at_4_decimals() {
        assert_eq!(proc_filters(&job(""), Some(1.5), None), vec!["volume=1.5000"]);
        assert_eq!(proc_filters(&job(""), Some(0.3333333), None), vec!["volume=0.3333"]);
    }

    #[test]
    fn fade_out_is_placed_from_duration() {
        assert_eq!(proc_filters(&job(r#", "fade": true"#), None, Some(10.0)),
                   vec!["afade=t=in:d=0.5", "afade=t=out:st=9.500:d=0.5"]);
    }

    #[test]
    fn fade_without_known_duration_only_fades_in() {
        assert_eq!(proc_filters(&job(r#", "fade": true"#), None, None),
                   vec!["afade=t=in:d=0.5"]);
    }

    #[test]
    fn fade_out_start_never_negative() {
        // Track shorter than the fade: fade-out starts at 0, not below
        assert_eq!(proc_filters(&job(r#", "fade": true, "fadeDur": 2.0"#), None, Some(1.0)),
                   vec!["afade=t=in:d=2", "afade=t=out:st=0.000:d=2"]);
    }

    #[test]
    fn full_chain_order_is_declip_hpf_gain_loudnorm_trim_fade() {
        // Filter ORDER is part of the audio contract: reordering changes output
        let j = job(r#", "declip": true, "hpf": true, "normalize": true, "trim": true, "fade": true"#);
        assert_eq!(proc_filters(&j, Some(2.0), Some(60.0)), vec![
            "adeclip=w=55:o=50",
            "highpass=f=80",
            "volume=2.0000",
            "loudnorm=I=-16:TP=-1.5:LRA=11",
            "silenceremove=start_periods=1:start_duration=0.3:start_threshold=-50dB:stop_periods=-1:stop_duration=0.3:stop_threshold=-50dB",
            "afade=t=in:d=0.5",
            "afade=t=out:st=59.500:d=0.5",
        ]);
    }
}
