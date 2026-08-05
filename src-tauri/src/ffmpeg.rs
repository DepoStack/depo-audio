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

const MAX_SIDECAR_STDOUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_SIDECAR_STDERR_BYTES: usize = 1024 * 1024;
const MAX_CONVERT_STDERR_BYTES: usize = 256 * 1024;

fn append_prefix_bounded(destination: &mut Vec<u8>, bytes: &[u8], max: usize) {
    let remaining = max.saturating_sub(destination.len());
    destination.extend_from_slice(&bytes[..bytes.len().min(remaining)]);
}

fn append_tail_bounded(destination: &mut Vec<u8>, bytes: &[u8], max: usize) {
    if bytes.len() >= max {
        destination.clear();
        destination.extend_from_slice(&bytes[bytes.len() - max..]);
        return;
    }
    let overflow = destination.len().saturating_add(bytes.len()).saturating_sub(max);
    if overflow > 0 {
        destination.drain(..overflow);
    }
    destination.extend_from_slice(bytes);
}

/// Optional cancellation predicate for a sidecar run (a user-cancelled scan).
/// Checked about once a second; on cancel the child is killed immediately
/// instead of decoding on until its timeout backstop.
pub(crate) type CancelCheck<'a> = Option<&'a (dyn Fn() -> bool + Sync)>;

/// Kill the child, then wait (bounded) for the plugin's wait-thread to reap
/// it. TerminateProcess on Windows returns before the process actually dies;
/// deleting the child's partial temp output before its handles close fails
/// with a sharing violation and silently leaks ~6MB per timed-out pass.
async fn kill_and_drain(
    child: Option<tauri_plugin_shell::process::CommandChild>,
    rx: &mut tauri::async_runtime::Receiver<CommandEvent>,
) {
    if let Some(c) = child {
        let _ = c.kill();
    }
    let grace = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let rem = grace.saturating_duration_since(tokio::time::Instant::now());
        if rem.is_zero() {
            return;
        }
        match tokio::time::timeout(rem, rx.recv()).await {
            Ok(Some(CommandEvent::Terminated(_))) | Ok(None) | Err(_) => return,
            Ok(Some(_)) => {}
        }
    }
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
    sidecar_output_cancellable(app, bin, args, secs, None).await
}

pub(crate) async fn sidecar_output_cancellable(
    app: &AppHandle,
    bin: &str,
    args: Vec<String>,
    secs: u64,
    cancelled: CancelCheck<'_>,
) -> Option<SidecarOutput> {
    let (mut rx, child) = app.shell().sidecar(bin).ok()?.args(args).spawn().ok()?;
    let mut child = Some(child);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(secs);
    let mut out = SidecarOutput {
        success: false,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    loop {
        if cancelled.map(|f| f()).unwrap_or(false) {
            kill_and_drain(child.take(), &mut rx).await;
            return None;
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            kill_and_drain(child.take(), &mut rx).await;
            return None;
        }
        // Wait in ~1s ticks so the cancel predicate is re-checked even while
        // a quiet child produces no events
        let tick = remaining.min(std::time::Duration::from_secs(1));
        match tokio::time::timeout(tick, rx.recv()).await {
            Err(_) => continue, // tick elapsed — re-check cancel + deadline
            Ok(None) => {
                kill_and_drain(child.take(), &mut rx).await;
                break;
            }
            Ok(Some(CommandEvent::Stdout(bytes))) => {
                append_prefix_bounded(&mut out.stdout, &bytes, MAX_SIDECAR_STDOUT_BYTES)
            }
            Ok(Some(CommandEvent::Stderr(bytes))) => {
                append_tail_bounded(&mut out.stderr, &bytes, MAX_SIDECAR_STDERR_BYTES)
            }
            Ok(Some(CommandEvent::Terminated(status))) => {
                child.take();
                out.success = status.code == Some(0);
                break;
            }
            Ok(Some(_)) => {}
        }
    }
    Some(out)
}

/// Parse FFmpeg's `-decoders` table without accepting substring matches.
/// Decoder names are the second whitespace-delimited field after the flags.
fn decoder_list_contains(output: &[u8], decoder: &str) -> bool {
    String::from_utf8_lossy(output).lines().any(|line| {
        let mut fields = line.split_whitespace();
        let flags = fields.next().unwrap_or_default();
        flags.len() == 6 && fields.next() == Some(decoder)
    })
}

/// Check the bundled FFmpeg once per process for its native FTR decoder.
/// Successful negative results are cached as well: changing the sidecar while
/// the app is running is unsupported, and a stable answer avoids one process
/// spawn per analysis pass.
pub(crate) async fn ftr_decoder_available(app: &AppHandle) -> Result<bool, String> {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    if let Some(available) = AVAILABLE.get() {
        return Ok(*available);
    }

    let output = sidecar_output_opt(
        app,
        ffmpeg_bin_name(),
        vec!["-hide_banner".into(), "-decoders".into()],
        15,
    )
    .await
    .ok_or_else(|| {
        "Could not inspect the bundled FFmpeg decoder list. Verify that FFmpeg is installed and executable.".to_string()
    })?;

    if !output.success {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("unknown FFmpeg error");
        return Err(format!("Could not inspect the bundled FFmpeg decoder list: {detail}"));
    }

    // FFmpeg normally writes the table to stdout, but accept stderr too for
    // compatibility with alternate static builds.
    let available = decoder_list_contains(&output.stdout, "ftr") || decoder_list_contains(&output.stderr, "ftr");
    let _ = AVAILABLE.set(available);
    Ok(available)
}

/// Return a user-actionable error before starting a TRM scan/conversion when
/// the bundled multimedia engine cannot support the format at all.
pub(crate) async fn ensure_ftr_decoder(app: &AppHandle) -> Result<(), String> {
    if ftr_decoder_available(app).await? {
        Ok(())
    } else {
        Err(
            "This FTR/TRM recording requires FFmpeg's native 'ftr' decoder, but this FFmpeg build does not provide it. Update or reinstall DepoAudio; forcing the ordinary AAC decoder cannot decode FTR's multichannel bitstream."
                .into(),
        )
    }
}

// Note: ffprobe auto-detects codecs and does not accept ffmpeg input options
// like -acodec, so the probe helpers take no input codec arguments.
pub(crate) async fn probe_duration(app: &AppHandle, feed: &Path) -> Option<f64> {
    probe_duration_cancellable(app, feed, None).await
}

pub(crate) async fn probe_duration_cancellable(
    app: &AppHandle,
    feed: &Path,
    cancelled: CancelCheck<'_>,
) -> Option<f64> {
    let args: Vec<String> = vec![
        "-v".into(),
        "quiet".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-print_format".into(),
        "json".into(),
        "-show_format".into(),
        feed.to_string_lossy().to_string(),
    ];
    let output = sidecar_output_cancellable(app, ffprobe_bin_name(), args, 30, cancelled).await?;
    let text = String::from_utf8_lossy(&output.stdout);
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["format"]["duration"].as_str()?.parse::<f64>().ok()
}

pub(crate) async fn probe_channels_cancellable(
    app: &AppHandle,
    feed: &Path,
    cancelled: CancelCheck<'_>,
) -> Option<u32> {
    let args: Vec<String> = vec![
        "-v".into(),
        "quiet".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-print_format".into(),
        "json".into(),
        "-show_streams".into(),
        "-select_streams".into(),
        "a:0".into(),
        feed.to_string_lossy().to_string(),
    ];
    let out = sidecar_output_cancellable(app, ffprobe_bin_name(), args, 30, cancelled).await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let v = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let ch = v["streams"][0]["channels"].as_u64()?;
    if ch > 0 {
        Some(ch as u32)
    } else {
        None
    }
}

// ── Filter chain builder ─────────────────────────────────────────────────────

/// Build the processing filter chain, optionally injecting a computed gain
/// value from auto-leveling analysis.
pub(crate) async fn build_proc_filters_with_gain(
    app: &AppHandle,
    opts: &ConvertJob,
    feed: &Path,
    auto_gain: Option<f64>,
    cancelled: CancelCheck<'_>,
) -> Vec<String> {
    // Duration is only needed to place the fade-out, so only probe then.
    let duration = if opts.fade {
        probe_duration_cancellable(app, feed, cancelled).await
    } else {
        None
    };
    proc_filters(opts, auto_gain, duration)
}

/// Pure core of the filter-chain builder: everything except the duration
/// probe. Filter order is part of the output contract (de-clip → HPF → gain →
/// loudnorm → source-relative fade-out → trim → fade-in) — see PARITY.md.
pub(crate) fn proc_filters(opts: &ConvertJob, auto_gain: Option<f64>, duration: Option<f64>) -> Vec<String> {
    let mut filters = Vec::new();

    // De-clipping runs first — reconstruct clipped peaks before other processing
    if opts.declip {
        filters.push("adeclip=w=55:o=50".into());
    }

    // High-pass filter removes low-frequency noise (HVAC, handling, rumble)
    if opts.hpf {
        filters.push(format!("highpass=f={}", opts.hpf_cutoff as u32));
    }

    // Auto-level gain injection (from analysis-computed per-channel gain)
    if let Some(gain) = auto_gain {
        if (gain - 1.0).abs() > 0.01 {
            filters.push(format!("volume={:.4}", gain));
        }
    }

    // Loudness normalization for consistent output level
    if opts.normalize {
        filters.push(format!(
            "loudnorm=I={}:TP={}:LRA=11",
            opts.normalize_lufs, opts.normalize_tp
        ));
    }

    // Place fade-out while timestamps still match the probed source duration.
    // Leading silence removal below shortens the result, so placing this after
    // trim can move the fade beyond EOF.
    if opts.fade {
        if let Some(d) = duration {
            let start = (d - opts.fade_dur).max(0.0);
            filters.push(format!("afade=t=out:st={start:.3}:d={}", opts.fade_dur));
        }
    }

    // `stop_periods=-1` removes every sufficiently long pause in the middle of
    // a recording, which changes the evidentiary timeline. A positive stop
    // period is not safe either: FFmpeg stops copying at the first qualifying
    // pause and cannot know that a pause is truly trailing until EOF. Keep trim
    // boundary-safe by removing leading dead air only.
    //
    // Split mode applies this filter chain independently to each channel. Even
    // leading-only trimming can then produce different start times, so retain
    // the original shared timeline for split outputs.
    if opts.trim && opts.mode != "split" {
        filters.push(format!(
            "silenceremove=start_periods=1:start_duration=0.3:start_threshold={}dB:start_mode=any",
            opts.silence_thresh
        ));
    }

    // Fade-in must follow leading trim so it applies to the audible start.
    if opts.fade {
        filters.push(format!("afade=t=in:d={}", opts.fade_dur));
    }
    filters
}

// ── Run FFmpeg sidecar ────────────────────────────────────────────────────────

/// Minimum allowed FFmpeg timeout, guarding against bogus persisted settings.
const MIN_FFMPEG_TIMEOUT_SECS: u64 = 30;
/// Maximum allowed FFmpeg timeout. A corrupt preference must not leave a
/// conversion child alive for days after the UI has stopped making progress.
const MAX_FFMPEG_TIMEOUT_SECS: u64 = 6 * 60 * 60;

fn bounded_ffmpeg_timeout(timeout_secs: u64) -> u64 {
    timeout_secs.clamp(MIN_FFMPEG_TIMEOUT_SECS, MAX_FFMPEG_TIMEOUT_SECS)
}

pub(crate) async fn run_ffmpeg_with_timeout(
    app: &AppHandle,
    args: Vec<String>,
    job_id: &str,
    timeout_secs: u64,
    total: Option<f64>,
    cancelled: CancelCheck<'_>,
) -> Result<(), String> {
    let (mut rx, child) = app
        .shell()
        .sidecar(ffmpeg_bin_name())
        .map_err(|e| e.to_string())?
        .args(args)
        .spawn()
        .map_err(|e| e.to_string())?;

    let mut stderr_acc = Vec::new();
    let time_re = time_regex();
    let timeout_secs = bounded_ffmpeg_timeout(timeout_secs);
    let started = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_secs);

    // Some(Some(0)) = clean exit; Some(_) = non-zero/signal kill; None = the
    // event stream closed without a Terminated event ever arriving.
    let mut exit_status: Option<Option<i32>> = None;
    let mut child = Some(child);

    loop {
        if cancelled.map(|check| check()).unwrap_or(false) {
            kill_and_drain(child.take(), &mut rx).await;
            return Err(crate::types::CONVERSION_CANCELLED_MESSAGE.into());
        }
        // Bound the wait so a silently wedged FFmpeg is still killed
        let remaining = match timeout.checked_sub(started.elapsed()) {
            Some(r) => r,
            None => {
                kill_and_drain(child.take(), &mut rx).await;
                return Err(format!(
                    "FFmpeg process timed out after {timeout_secs} seconds and was killed"
                ));
            }
        };
        // A quiet encoder may emit nothing for a long stretch. Wake at least
        // four times per second so Cancel terminates it promptly rather than
        // waiting for stderr progress or the full timeout.
        let tick = remaining.min(std::time::Duration::from_millis(250));
        let event = match tokio::time::timeout(tick, rx.recv()).await {
            Ok(Some(event)) => event,
            Ok(None) => {
                kill_and_drain(child.take(), &mut rx).await;
                break;
            }
            Err(_) => continue,
        };

        match event {
            CommandEvent::Stderr(bytes) => {
                let line = String::from_utf8_lossy(&bytes).to_string();
                append_tail_bounded(&mut stderr_acc, &bytes, MAX_CONVERT_STDERR_BYTES);
                if let Some(cap) = time_re.captures(&line) {
                    let h: f64 = cap[1].parse().unwrap_or(0.0);
                    let m: f64 = cap[2].parse().unwrap_or(0.0);
                    let s: f64 = cap[3].parse().unwrap_or(0.0);
                    let secs = h * 3600.0 + m * 60.0 + s;
                    let _ = app.emit(
                        "convert:progress",
                        ProgressEvent {
                            id: job_id.to_string(),
                            seconds: secs,
                            phase: None,
                            total,
                        },
                    );
                }
            }
            CommandEvent::Terminated(status) => {
                child.take();
                exit_status = Some(status.code);
                break;
            }
            _ => {}
        }
    }

    // Honor a cancel racing the final successful termination event. The
    // caller's output cleanup guard will remove the just-finished file.
    if cancelled.map(|check| check()).unwrap_or(false) {
        return Err(crate::types::CONVERSION_CANCELLED_MESSAGE.into());
    }

    match exit_status {
        Some(Some(0)) => Ok(()),
        // Non-zero exit OR a signal kill (code == None): surface the tail of
        // stderr so the failure isn't reported as success.
        Some(_) => {
            let stderr_text = String::from_utf8_lossy(&stderr_acc);
            let lines: Vec<&str> = stderr_text
                .lines()
                .filter(|l| {
                    !l.starts_with("ffmpeg version")
                        && !l.starts_with("built")
                        && !l.starts_with("lib")
                        && !l.starts_with("configuration:")
                })
                .collect();
            let msg = lines
                .iter()
                .rev()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join(" | ");
            // Sanitize: strip file paths from error messages
            let sanitized = msg.replace(['/', '\\'], "_").chars().take(300).collect::<String>();
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

    #[test]
    fn sidecar_output_buffers_are_bounded() {
        let mut prefix = b"abc".to_vec();
        append_prefix_bounded(&mut prefix, b"defghi", 5);
        assert_eq!(prefix, b"abcde");

        let mut tail = b"abc".to_vec();
        append_tail_bounded(&mut tail, b"defghi", 5);
        assert_eq!(tail, b"efghi");
    }

    #[test]
    fn decoder_inventory_matches_the_exact_ftr_decoder() {
        let inventory = br#"
Decoders:
 V..... h264                 H.264 / AVC
 A....D aac                  AAC (Advanced Audio Coding)
 A....D ftr                  FTR Voice
 A....D ftr_variant          Not the exact decoder
"#;

        assert!(decoder_list_contains(inventory, "ftr"));
        assert!(decoder_list_contains(inventory, "aac"));
        assert!(!decoder_list_contains(inventory, "tr"));
        assert!(!decoder_list_contains(inventory, "ftr_voice"));
    }

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
        let overrides: serde_json::Value = serde_json::from_str(&format!("{{{}}}", frag)).expect("valid override JSON");
        if let (Some(b), serde_json::Value::Object(o)) = (base.as_object_mut(), overrides) {
            for (k, v) in o {
                b.insert(k, v);
            }
        }
        serde_json::from_value(base).expect("valid job JSON")
    }

    #[test]
    fn all_processing_off_yields_empty_chain() {
        assert!(proc_filters(&job(""), None, None).is_empty());
    }

    #[test]
    fn declip_filter_is_stable() {
        assert_eq!(
            proc_filters(&job(r#", "declip": true"#), None, None),
            vec!["adeclip=w=55:o=50"]
        );
    }

    #[test]
    fn hpf_uses_default_cutoff_80() {
        assert_eq!(
            proc_filters(&job(r#", "hpf": true"#), None, None),
            vec!["highpass=f=80"]
        );
    }

    #[test]
    fn hpf_honors_custom_cutoff() {
        assert_eq!(
            proc_filters(&job(r#", "hpf": true, "hpfCutoff": 120.0"#), None, None),
            vec!["highpass=f=120"]
        );
    }

    #[test]
    fn normalize_uses_default_lufs_and_tp() {
        assert_eq!(
            proc_filters(&job(r#", "normalize": true"#), None, None),
            vec!["loudnorm=I=-16:TP=-1.5:LRA=11"]
        );
    }

    #[test]
    fn trim_uses_default_silence_threshold() {
        assert_eq!(
            proc_filters(&job(r#", "trim": true"#), None, None),
            vec!["silenceremove=start_periods=1:start_duration=0.3:start_threshold=-50dB:start_mode=any"]
        );
    }

    #[test]
    fn trim_never_removes_interior_silence() {
        let filters = proc_filters(&job(r#", "trim": true"#), None, None);
        assert_eq!(filters.len(), 1);
        assert!(!filters[0].contains("stop_periods"));
    }

    #[test]
    fn split_trim_preserves_the_shared_channel_timeline() {
        let filters = proc_filters(&job(r#", "trim": true, "mode": "split""#), None, None);
        assert!(filters.is_empty());
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
        assert_eq!(
            proc_filters(&job(r#", "fade": true"#), None, Some(10.0)),
            vec!["afade=t=out:st=9.500:d=0.5", "afade=t=in:d=0.5"]
        );
    }

    #[test]
    fn fade_without_known_duration_only_fades_in() {
        assert_eq!(
            proc_filters(&job(r#", "fade": true"#), None, None),
            vec!["afade=t=in:d=0.5"]
        );
    }

    #[test]
    fn fade_out_start_never_negative() {
        // Track shorter than the fade: fade-out starts at 0, not below
        assert_eq!(
            proc_filters(&job(r#", "fade": true, "fadeDur": 2.0"#), None, Some(1.0)),
            vec!["afade=t=out:st=0.000:d=2", "afade=t=in:d=2"]
        );
    }

    #[test]
    fn trim_and_fade_order_keeps_fade_out_on_source_timeline() {
        assert_eq!(
            proc_filters(&job(r#", "trim": true, "fade": true"#), None, Some(10.0)),
            vec![
                "afade=t=out:st=9.500:d=0.5",
                "silenceremove=start_periods=1:start_duration=0.3:start_threshold=-50dB:start_mode=any",
                "afade=t=in:d=0.5",
            ]
        );
    }

    #[test]
    fn full_chain_order_is_declip_hpf_gain_loudnorm_fade_out_trim_fade_in() {
        // Filter ORDER is part of the audio contract: reordering changes output
        let j = job(r#", "declip": true, "hpf": true, "normalize": true, "trim": true, "fade": true"#);
        assert_eq!(
            proc_filters(&j, Some(2.0), Some(60.0)),
            vec![
                "adeclip=w=55:o=50",
                "highpass=f=80",
                "volume=2.0000",
                "loudnorm=I=-16:TP=-1.5:LRA=11",
                "afade=t=out:st=59.500:d=0.5",
                "silenceremove=start_periods=1:start_duration=0.3:start_threshold=-50dB:start_mode=any",
                "afade=t=in:d=0.5",
            ]
        );
    }

    #[test]
    fn ffmpeg_timeout_is_bounded_in_both_directions() {
        assert_eq!(bounded_ffmpeg_timeout(0), MIN_FFMPEG_TIMEOUT_SECS);
        assert_eq!(bounded_ffmpeg_timeout(300), 300);
        assert_eq!(bounded_ffmpeg_timeout(u64::MAX), MAX_FFMPEG_TIMEOUT_SECS);
    }
}
