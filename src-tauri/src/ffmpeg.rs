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
pub(crate) async fn probe_duration_cancellable(
    app: &AppHandle,
    feed: &Path,
    cancelled: CancelCheck<'_>,
) -> Option<f64> {
    let output = sidecar_output_cancellable(app, ffprobe_bin_name(), duration_probe_args(feed), 30, cancelled).await?;
    parse_duration_probe(&output.stdout)
}

/// ffprobe arguments for the duration of the converted stream (`a:0`, the
/// stream every FFmpeg pass maps), with the container duration as fallback.
/// A video with a shorter first audio track, or a file with a longer second
/// track, reports a container duration past the end of the converted stream.
pub(crate) fn duration_probe_args(feed: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "quiet".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-print_format".into(),
        "json".into(),
        "-select_streams".into(),
        crate::helpers::source_audio_stream_specifier(),
        "-show_entries".into(),
        "stream=duration:stream_tags=DURATION:format=duration".into(),
        feed.to_string_lossy().to_string(),
    ]
}

/// Duration in seconds from `duration_probe_args` output, if readable: the
/// converted stream's own duration, else the container's.
pub(crate) fn parse_duration_probe(stdout: &[u8]) -> Option<f64> {
    let text = String::from_utf8_lossy(stdout);
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    crate::manifest::converted_stream_duration(&v)
}

pub(crate) async fn probe_channels_cancellable(
    app: &AppHandle,
    feed: &Path,
    cancelled: CancelCheck<'_>,
) -> Option<u32> {
    let out = sidecar_output_cancellable(app, ffprobe_bin_name(), channel_probe_args(feed), 30, cancelled).await?;
    parse_channel_probe(&out.stdout)
}

/// ffprobe arguments for the channel count of the converted audio stream
/// (`SOURCE_AUDIO_STREAM`, the same stream every FFmpeg pass maps).
pub(crate) fn channel_probe_args(feed: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "quiet".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-print_format".into(),
        "json".into(),
        "-show_streams".into(),
        "-select_streams".into(),
        crate::helpers::source_audio_stream_specifier(),
        feed.to_string_lossy().to_string(),
    ]
}

/// Channel count from `channel_probe_args` output; zero or unreadable is None.
pub(crate) fn parse_channel_probe(stdout: &[u8]) -> Option<u32> {
    let text = String::from_utf8_lossy(stdout);
    let v = serde_json::from_str::<serde_json::Value>(&text).ok()?;
    let ch = v["streams"][0]["channels"].as_u64()?;
    if ch > 0 {
        Some(ch as u32)
    } else {
        None
    }
}

/// Channel count, sample rate and BWF bext TimeReference of the conversion
/// input, for the export manifest (DA-08). A failed probe yields an empty
/// record: the manifest then says the start is unknown instead of guessing.
pub(crate) async fn probe_manifest_source_cancellable(
    app: &AppHandle,
    feed: &Path,
    cancelled: CancelCheck<'_>,
) -> crate::manifest::ProbedSource {
    let Some(out) = sidecar_output_cancellable(app, ffprobe_bin_name(), manifest_probe_args(feed), 30, cancelled).await
    else {
        return crate::manifest::ProbedSource::default();
    };
    serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .map(|probe| crate::manifest::parse_probe(&probe))
        .unwrap_or_default()
}

/// ffprobe arguments for the export record's source facts: format tags and
/// EVERY audio stream, in file order, so `streams[SOURCE_AUDIO_STREAM]` is the
/// converted stream and the array length is the input's audio stream count.
/// The Convert queue's multi-track notice reads the same document.
pub(crate) fn manifest_probe_args(feed: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "quiet".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-print_format".into(),
        "json".into(),
        "-show_format".into(),
        "-show_streams".into(),
        "-select_streams".into(),
        "a".into(),
        feed.to_string_lossy().to_string(),
    ]
}

/// Audio streams of a queued file for the Convert queue's notice (the same
/// ffprobe document the export record reads). None when ffprobe fails.
pub(crate) async fn probe_audio_streams(app: &AppHandle, feed: &Path) -> Option<crate::manifest::AudioStreamsInfo> {
    let out = sidecar_output_opt(app, ffprobe_bin_name(), manifest_probe_args(feed), 30).await?;
    if !out.success {
        return None;
    }
    let probe = serde_json::from_slice::<serde_json::Value>(&out.stdout).ok()?;
    Some(crate::manifest::parse_audio_streams(&probe))
}

// ── Filter chain builder ─────────────────────────────────────────────────────

/// Sample rate the decoded-length pass counts samples at.
pub(crate) const DECODED_LENGTH_RATE: u32 = 48_000;

/// The decoded-length pass's filter: resample to `DECODED_LENGTH_RATE` and
/// count the samples. The release workflow runs this exact string on the
/// bundled FFmpeg (scripts/release-contract-check.mjs keeps it equal).
pub(crate) const DECODED_LENGTH_FILTER: &str =
    "aresample=48000,astats=metadata=0:reset=0:measure_perchannel=none:measure_overall=Number_of_samples";

/// Why a conversion with Fade on stops when the decoded length is unknown.
pub(crate) const FADE_LENGTH_UNKNOWN_MESSAGE: &str = "Fade is on, but DepoAudio could not measure how long the recording decodes to, so it cannot place the fade-out at the real end. Nothing was converted. Turn Fade off, or try again.";

/// FFmpeg arguments that decode the converted stream exactly as a conversion
/// does (same input decoder, `-map 0:a:0`) and count its samples at
/// `DECODED_LENGTH_RATE`. The count is the length that is really converted,
/// which a stated duration may not be: a chained Ogg file states its last
/// link's length, and a VBR MP3 without an index states an estimate.
pub(crate) fn decoded_length_args(feed: &Path) -> Vec<String> {
    let mut args = crate::helpers::safe_ffmpeg_input_prelude();
    args.extend(["-nostats".into(), "-v".into(), "info".into()]);
    args.extend(crate::helpers::input_codec_args(feed));
    args.extend(["-i".into(), feed.to_string_lossy().to_string()]);
    args.extend(crate::helpers::source_audio_map_args());
    args.extend([
        "-vn".into(),
        "-sn".into(),
        "-af".into(),
        DECODED_LENGTH_FILTER.into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);
    args
}

/// Decoded length in seconds from the stderr of `decoded_length_args`: the
/// last "Number of samples" astats reports, over `DECODED_LENGTH_RATE`.
pub(crate) fn parse_decoded_seconds(stderr: &[u8]) -> Option<f64> {
    let text = String::from_utf8_lossy(stderr);
    let samples: u64 = text
        .lines()
        .rev()
        .find_map(|line| line.split("Number of samples:").nth(1)?.trim().parse().ok())?;
    (samples > 0).then(|| samples as f64 / f64::from(DECODED_LENGTH_RATE))
}

pub(crate) async fn measure_decoded_seconds_cancellable(
    app: &AppHandle,
    feed: &Path,
    timeout_secs: u64,
    cancelled: CancelCheck<'_>,
) -> Option<f64> {
    let output = sidecar_output_cancellable(
        app,
        ffmpeg_bin_name(),
        decoded_length_args(feed),
        bounded_ffmpeg_timeout(timeout_secs),
        cancelled,
    )
    .await?;
    if !output.success {
        return None;
    }
    parse_decoded_seconds(&output.stderr)
}

// ── Leading-silence trim measurement ─────────────────────────────────────────

/// astats options that count the samples per channel and print the total.
const SAMPLE_COUNT_STATS: &str = "metadata=0:reset=0:measure_perchannel=none:measure_overall=Number_of_samples";

/// The trim detection: FFmpeg's leading-silence removal, stopping at the
/// first sample above the threshold on any channel
/// (`detection=peak:window=0`), so only audio below the threshold is
/// removed. It runs only in the measuring pass
/// (`leading_trim_measure_filter`); the conversion itself removes the
/// measured number of samples with `leading_trim_filters`.
///
/// Leading only: `stop_periods=-1` would remove every long pause in the
/// middle of a recording, and a positive stop period stops copying at the
/// first qualifying pause. FFmpeg 6.1 and newer also drop the
/// `start_duration` of sound they use to confirm the silence has ended, and
/// the default 20 ms RMS window ends the silence a few samples late, so
/// neither is used: an earlier `start_duration=0.3` cut the first 0.3 s of
/// speech.
pub(crate) fn trim_detection_filter(silence_thresh: f64) -> String {
    format!(
        "silenceremove=start_periods=1:start_threshold={}dB:start_mode=any:detection=peak:window=0",
        silence_thresh
    )
}

/// The measuring pass's filter: count every decoded sample, run the trim
/// detection, and count what is left. The difference is exactly the number
/// of samples trim removes. The release workflow runs this string (at the
/// default -50 dB) on the bundled FFmpeg.
pub(crate) fn leading_trim_measure_filter(silence_thresh: f64) -> String {
    format!(
        "astats@total={SAMPLE_COUNT_STATS},{},astats@kept={SAMPLE_COUNT_STATS}",
        trim_detection_filter(silence_thresh)
    )
}

/// The filters that remove exactly `samples` samples from the start: number
/// the samples from 0 by count (`N`, never the decoder's timestamps), keep
/// from sample `samples` on, and restart the timestamps at 0 for the filters
/// after it. They come first in the chain, before any filter that changes
/// the sample rate (loudnorm runs at 192 kHz), so `samples` counts at the
/// converted stream's own rate.
pub(crate) fn leading_trim_filters(samples: u64) -> Vec<String> {
    vec![
        "asetpts=N/SR/TB".into(),
        format!("atrim=start_sample={samples}"),
        "asetpts=PTS-STARTPTS".into(),
    ]
}

/// FFmpeg arguments for the measuring pass: the converted stream decoded
/// exactly as a conversion decodes it (same input decoder, `-map 0:a:0`),
/// at its own rate.
pub(crate) fn leading_trim_measure_args(feed: &Path, silence_thresh: f64) -> Vec<String> {
    let mut args = crate::helpers::safe_ffmpeg_input_prelude();
    args.extend(["-nostats".into(), "-v".into(), "info".into()]);
    args.extend(crate::helpers::input_codec_args(feed));
    args.extend(["-i".into(), feed.to_string_lossy().to_string()]);
    args.extend(crate::helpers::source_audio_map_args());
    args.extend([
        "-vn".into(),
        "-sn".into(),
        "-af".into(),
        leading_trim_measure_filter(silence_thresh),
        "-f".into(),
        "null".into(),
        "-".into(),
    ]);
    args
}

/// The two counts from the stderr of a successful `leading_trim_measure_args`
/// run: (decoded samples, samples left after the trim detection), each the
/// last count its astats instance reports. astats reports nothing when no
/// sample reaches it, so a run whose second instance printed nothing at all
/// kept 0 samples (checked with FFmpeg 6.1.1 on a silent file). None unless
/// the first count is present and the second is not larger than it.
pub(crate) fn parse_leading_trim_counts(stderr: &[u8]) -> Option<(u64, u64)> {
    let text = String::from_utf8_lossy(stderr);
    let count = |instance: &str| -> Option<u64> {
        let prefix = format!("[astats@{instance} @");
        text.lines()
            .rev()
            .filter(|line| line.starts_with(&prefix))
            .find_map(|line| line.split("Number of samples:").nth(1)?.trim().parse().ok())
    };
    let total = count("total")?;
    let kept = match count("kept") {
        Some(kept) => kept,
        None if !text.contains("[astats@kept @") => 0,
        None => return None,
    };
    (kept <= total).then_some((total, kept))
}

/// Why a conversion with Trim silence on stops when the trim is not measured.
pub(crate) const TRIM_UNMEASURED_MESSAGE: &str = "Trim silence is on, but DepoAudio could not measure how much silence is at the start, so it cannot record what trim removes. Nothing was converted. Turn Trim silence off, or try again.";

/// The measured trim for a stream at `sample_rate`, as the export record
/// holds it. An all-silent recording is refused rather than converted to an
/// empty file.
pub(crate) fn leading_trim_record(
    opts: &ConvertJob,
    counts: (u64, u64),
    sample_rate: u32,
) -> Result<crate::manifest::LeadingTrimRecord, String> {
    let (total, kept) = counts;
    if kept == 0 {
        return Err(format!(
            "Trim silence is on, and every sample of this recording is below the silence threshold ({} dB), so trim would remove all of it. Nothing was converted. Turn Trim silence off, or lower the threshold in Settings.",
            opts.silence_thresh
        ));
    }
    let samples = total - kept;
    Ok(crate::manifest::LeadingTrimRecord {
        samples,
        seconds: samples as f64 / f64::from(sample_rate),
        sample_rate,
        threshold_db: opts.silence_thresh,
        decoded_source_samples: total,
        measured_with: leading_trim_measure_filter(opts.silence_thresh),
    })
}

/// Measure leading-silence trim, when it applies to this conversion (Trim
/// silence on, and not Split Channels, which keeps the shared timeline).
pub(crate) async fn measure_leading_trim_cancellable(
    app: &AppHandle,
    opts: &ConvertJob,
    feed: &Path,
    sample_rate: Option<u32>,
    cancelled: CancelCheck<'_>,
) -> Result<Option<crate::manifest::LeadingTrimRecord>, String> {
    if !trim_applies(opts) {
        return Ok(None);
    }
    let rate = sample_rate.ok_or(TRIM_UNMEASURED_MESSAGE)?;
    let output = sidecar_output_cancellable(
        app,
        ffmpeg_bin_name(),
        leading_trim_measure_args(feed, opts.silence_thresh),
        bounded_ffmpeg_timeout(u64::from(opts.ffmpeg_timeout)),
        cancelled,
    )
    .await
    .ok_or(TRIM_UNMEASURED_MESSAGE)?;
    if !output.success {
        return Err(TRIM_UNMEASURED_MESSAGE.into());
    }
    let counts = parse_leading_trim_counts(&output.stderr).ok_or(TRIM_UNMEASURED_MESSAGE)?;
    leading_trim_record(opts, counts, rate).map(Some)
}

/// Trim silence applies to Mix to Stereo and Keep Original. Split Channels
/// applies the chain to each channel on its own, where trim could start the
/// channels at different times, so it keeps the shared timeline instead.
pub(crate) fn trim_applies(opts: &ConvertJob) -> bool {
    opts.trim && opts.mode != "split"
}

/// Where the fade-out goes on a recording that decodes to `decoded` seconds,
/// of which leading-silence trim removes `trimmed` seconds from the start,
/// with the length its header states (`stated`) kept for the record. The
/// start is on the output's timeline.
pub(crate) fn fade_out_placement(
    opts: &ConvertJob,
    decoded: f64,
    trimmed: f64,
    stated: Option<f64>,
) -> crate::manifest::FadeOutRecord {
    let start = (decoded - trimmed - opts.fade_dur).max(0.0);
    crate::manifest::FadeOutRecord {
        start_seconds: (start * 1000.0).round() / 1000.0,
        duration_seconds: opts.fade_dur,
        decoded_source_seconds: decoded,
        stated_source_seconds: stated,
    }
}

/// The processing filters of one conversion and what the export record
/// says about them.
pub(crate) struct ProcPlan {
    pub filters: Vec<String>,
    pub fade_out: Option<crate::manifest::FadeOutRecord>,
    pub leading_trim: Option<crate::manifest::LeadingTrimRecord>,
}

/// Build the processing filter chain, optionally injecting a computed gain
/// value from auto-leveling analysis. With Trim silence on, a measuring pass
/// counts the samples trim removes (`measure_leading_trim_cancellable`) and
/// the chain removes exactly that many. With Fade on, the recording is
/// decoded once to measure its real length, and the fade-out is placed from
/// that, never from the stated duration (`stated`, kept for the record
/// only): a stated length shorter than the audio put the fade-out in the
/// middle and silenced everything after it. If either cannot be measured the
/// conversion stops rather than guess. `sample_rate` is the converted
/// stream's rate, which the trim counts at.
pub(crate) async fn build_proc_filters_with_gain(
    app: &AppHandle,
    opts: &ConvertJob,
    feed: &Path,
    auto_gain: Option<f64>,
    stated: Option<f64>,
    sample_rate: Option<u32>,
    cancelled: CancelCheck<'_>,
) -> Result<ProcPlan, String> {
    let leading_trim = measure_leading_trim_cancellable(app, opts, feed, sample_rate, cancelled).await?;
    let trimmed = leading_trim.as_ref().map_or(0.0, |trim| trim.seconds);
    let fade_out = if opts.fade {
        let decoded = measure_decoded_seconds_cancellable(app, feed, u64::from(opts.ffmpeg_timeout), cancelled)
            .await
            .ok_or(FADE_LENGTH_UNKNOWN_MESSAGE)?;
        Some(fade_out_placement(opts, decoded, trimmed, stated))
    } else {
        None
    };
    Ok(proc_plan(opts, auto_gain, fade_out, leading_trim))
}

/// The filters for measured trim and fade placements (no process runs).
pub(crate) fn proc_plan(
    opts: &ConvertJob,
    auto_gain: Option<f64>,
    fade_out: Option<crate::manifest::FadeOutRecord>,
    leading_trim: Option<crate::manifest::LeadingTrimRecord>,
) -> ProcPlan {
    let length = fade_out
        .as_ref()
        .map(|fade| fade.decoded_source_seconds - leading_trim.as_ref().map_or(0.0, |trim| trim.seconds));
    ProcPlan {
        filters: proc_filters(opts, auto_gain, length, leading_trim.as_ref().map(|trim| trim.samples)),
        fade_out,
        leading_trim,
    }
}

/// Pure core of the filter-chain builder: everything except the
/// measurements. `duration` is the length of the audio after trim (the
/// decoded length less what trim removes), which places the fade-out;
/// `trim_samples` is the measured leading-silence trim. Filter order is part
/// of the output contract (trim → de-clip → HPF → gain → loudnorm →
/// fade-out → fade-in) — see PARITY.md.
pub(crate) fn proc_filters(
    opts: &ConvertJob,
    auto_gain: Option<f64>,
    duration: Option<f64>,
    trim_samples: Option<u64>,
) -> Vec<String> {
    let mut filters = Vec::new();

    // Leading-silence trim runs first, while the samples are still at the
    // converted stream's own rate, and removes exactly the measured count.
    // Only leading audio below the threshold is removed: interior pauses
    // and the ending are kept. Split Channels never trims (`trim_applies`).
    if trim_applies(opts) {
        if let Some(samples) = trim_samples.filter(|samples| *samples > 0) {
            filters.extend(leading_trim_filters(samples));
        }
    }

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

    // The fade-out is placed from the length after trim, on the timeline the
    // trim filters restart at 0.
    if opts.fade {
        if let Some(d) = duration {
            let start = (d - opts.fade_dur).max(0.0);
            filters.push(format!("afade=t=out:st={start:.3}:d={}", opts.fade_dur));
        }
    }

    // Fade-in follows leading trim so it applies to the audible start.
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
        assert!(proc_filters(&job(""), None, None, None).is_empty());
    }

    #[test]
    fn declip_filter_is_stable() {
        assert_eq!(
            proc_filters(&job(r#", "declip": true"#), None, None, None),
            vec!["adeclip=w=55:o=50"]
        );
    }

    #[test]
    fn hpf_uses_default_cutoff_80() {
        assert_eq!(
            proc_filters(&job(r#", "hpf": true"#), None, None, None),
            vec!["highpass=f=80"]
        );
    }

    #[test]
    fn hpf_honors_custom_cutoff() {
        assert_eq!(
            proc_filters(&job(r#", "hpf": true, "hpfCutoff": 120.0"#), None, None, None),
            vec!["highpass=f=120"]
        );
    }

    #[test]
    fn normalize_uses_default_lufs_and_tp() {
        assert_eq!(
            proc_filters(&job(r#", "normalize": true"#), None, None, None),
            vec!["loudnorm=I=-16:TP=-1.5:LRA=11"]
        );
    }

    #[test]
    fn trim_removes_exactly_the_measured_samples_by_count() {
        assert_eq!(
            proc_filters(&job(r#", "trim": true"#), None, None, Some(24_002)),
            vec!["asetpts=N/SR/TB", "atrim=start_sample=24002", "asetpts=PTS-STARTPTS"]
        );
        // Nothing measured, or nothing to remove: no trim filters.
        assert!(proc_filters(&job(r#", "trim": true"#), None, None, None).is_empty());
        assert!(proc_filters(&job(r#", "trim": true"#), None, None, Some(0)).is_empty());
        // Trim off: a stray count is ignored.
        assert!(proc_filters(&job(""), None, None, Some(10)).is_empty());
    }

    #[test]
    fn trim_detection_uses_the_threshold_and_never_removes_interior_silence() {
        assert_eq!(
            trim_detection_filter(-50.0),
            "silenceremove=start_periods=1:start_threshold=-50dB:start_mode=any:detection=peak:window=0"
        );
        assert_eq!(
            trim_detection_filter(-42.5),
            "silenceremove=start_periods=1:start_threshold=-42.5dB:start_mode=any:detection=peak:window=0"
        );
        assert!(!trim_detection_filter(-50.0).contains("stop_periods"));
        assert_eq!(
            leading_trim_measure_filter(-50.0),
            "astats@total=metadata=0:reset=0:measure_perchannel=none:measure_overall=Number_of_samples,\
silenceremove=start_periods=1:start_threshold=-50dB:start_mode=any:detection=peak:window=0,\
astats@kept=metadata=0:reset=0:measure_perchannel=none:measure_overall=Number_of_samples"
        );
    }

    #[test]
    fn trim_measuring_pass_decodes_the_converted_stream_at_its_own_rate() {
        let args = leading_trim_measure_args(Path::new("/in/session.sgmca"), -50.0);
        let joined = args.join(" ");
        assert!(
            joined.contains("-c:a libopus -i /in/session.sgmca -map 0:a:0 -vn -sn -af astats@total="),
            "{joined}"
        );
        assert!(!args.iter().any(|arg| arg == "-ar" || arg.contains("aresample")));
        assert!(joined.ends_with("-f null -"));
    }

    #[test]
    fn trim_counts_are_read_per_astats_instance() {
        let log = b"[astats@total @ 0x1] Overall\n\
[astats@total @ 0x1] Number of samples: 144000\n\
[astats@kept @ 0x2] Overall\n\
[astats@kept @ 0x2] Number of samples: 119999\n";
        assert_eq!(parse_leading_trim_counts(log), Some((144_000, 119_999)));
        // Nothing reached the second count: every sample was trimmed.
        assert_eq!(
            parse_leading_trim_counts(b"[astats@total @ 0x1] Number of samples: 10\n"),
            Some((10, 0))
        );
        assert_eq!(
            parse_leading_trim_counts(b"[astats@total @ 0x1] Number of samples: 10\n[astats@kept @ 0x2] Overall\n"),
            None
        );
        assert_eq!(
            parse_leading_trim_counts(b"[astats@kept @ 0x2] Number of samples: 10\n"),
            None
        );
        // More kept than decoded cannot be a trim.
        assert_eq!(
            parse_leading_trim_counts(
                b"[astats@total @ 0x1] Number of samples: 10\n[astats@kept @ 0x2] Number of samples: 11\n"
            ),
            None
        );
    }

    #[test]
    fn trim_record_holds_the_exact_count_and_refuses_an_all_silent_recording() {
        let opts = job(r#", "trim": true"#);
        let record = leading_trim_record(&opts, (144_000, 119_999), 48_000).unwrap();
        assert_eq!(record.samples, 24_001);
        assert_eq!(record.seconds, 24_001.0 / 48_000.0);
        assert_eq!(record.sample_rate, 48_000);
        assert_eq!(record.threshold_db, -50.0);
        assert_eq!(record.decoded_source_samples, 144_000);
        assert_eq!(record.measured_with, leading_trim_measure_filter(-50.0));
        let error = leading_trim_record(&opts, (144_000, 0), 48_000).unwrap_err();
        assert!(error.contains("every sample of this recording is below the silence threshold (-50 dB)"));
    }

    #[test]
    fn split_trim_preserves_the_shared_channel_timeline() {
        let split = job(r#", "trim": true, "mode": "split""#);
        assert!(!trim_applies(&split));
        assert!(proc_filters(&split, None, None, Some(100)).is_empty());
        assert!(trim_applies(&job(r#", "trim": true, "mode": "keep""#)));
    }

    #[test]
    fn near_unity_auto_gain_is_skipped() {
        // Gains within ±0.01 of 1.0 are noise, not leveling — omitted entirely
        assert!(proc_filters(&job(""), Some(1.005), None, None).is_empty());
        assert!(proc_filters(&job(""), Some(0.995), None, None).is_empty());
    }

    #[test]
    fn auto_gain_is_injected_at_4_decimals() {
        assert_eq!(proc_filters(&job(""), Some(1.5), None, None), vec!["volume=1.5000"]);
        assert_eq!(
            proc_filters(&job(""), Some(0.3333333), None, None),
            vec!["volume=0.3333"]
        );
    }

    #[test]
    fn fade_out_is_placed_from_duration() {
        assert_eq!(
            proc_filters(&job(r#", "fade": true"#), None, Some(10.0), None),
            vec!["afade=t=out:st=9.500:d=0.5", "afade=t=in:d=0.5"]
        );
    }

    #[test]
    fn fade_without_known_duration_only_fades_in() {
        assert_eq!(
            proc_filters(&job(r#", "fade": true"#), None, None, None),
            vec!["afade=t=in:d=0.5"]
        );
    }

    #[test]
    fn fade_out_start_never_negative() {
        // Track shorter than the fade: fade-out starts at 0, not below
        assert_eq!(
            proc_filters(&job(r#", "fade": true, "fadeDur": 2.0"#), None, Some(1.0), None),
            vec!["afade=t=out:st=0.000:d=2", "afade=t=in:d=2"]
        );
    }

    #[test]
    fn trim_comes_first_and_the_fade_out_is_placed_on_the_trimmed_timeline() {
        let opts = job(r#", "trim": true, "fade": true"#);
        // 10 s decoded, 0.5 s trimmed: 9.5 s remain, so the fade-out starts at 9 s.
        let trim = leading_trim_record(&opts, (480_000, 456_000), 48_000).unwrap();
        let fade = fade_out_placement(&opts, 10.0, trim.seconds, None);
        assert_eq!(fade.start_seconds, 9.0);
        let plan = proc_plan(&opts, None, Some(fade), Some(trim));
        assert_eq!(
            plan.filters,
            vec![
                "asetpts=N/SR/TB",
                "atrim=start_sample=24000",
                "asetpts=PTS-STARTPTS",
                "afade=t=out:st=9.000:d=0.5",
                "afade=t=in:d=0.5",
            ]
        );
    }

    #[test]
    fn full_chain_order_is_trim_declip_hpf_gain_loudnorm_fade_out_fade_in() {
        // Filter ORDER is part of the audio contract: reordering changes output
        let j = job(r#", "declip": true, "hpf": true, "normalize": true, "trim": true, "fade": true"#);
        assert_eq!(
            proc_filters(&j, Some(2.0), Some(60.0), Some(480)),
            vec![
                "asetpts=N/SR/TB",
                "atrim=start_sample=480",
                "asetpts=PTS-STARTPTS",
                "adeclip=w=55:o=50",
                "highpass=f=80",
                "volume=2.0000",
                "loudnorm=I=-16:TP=-1.5:LRA=11",
                "afade=t=out:st=59.500:d=0.5",
                "afade=t=in:d=0.5",
            ]
        );
    }

    #[test]
    fn decoded_length_pass_maps_the_converted_stream_and_counts_at_48_khz() {
        let args = decoded_length_args(Path::new("/in/session.sgmca"));
        let joined = args.join(" ");
        assert!(
            joined.contains("-c:a libopus -i /in/session.sgmca -map 0:a:0 -vn -sn"),
            "{joined}"
        );
        assert!(joined.contains(
            "-af aresample=48000,astats=metadata=0:reset=0:measure_perchannel=none:measure_overall=Number_of_samples -f null -"
        ));
        assert_eq!(&args[..4], &["-nostdin", "-hide_banner", "-protocol_whitelist", "file"]);
        assert!(DECODED_LENGTH_FILTER.starts_with(&format!("aresample={DECODED_LENGTH_RATE},")));
    }

    #[test]
    fn decoded_length_is_the_last_sample_count_astats_reports() {
        let log = b"[aist#0:0/opus @ 0x1] timestamp discontinuity (stream id=0): -10000000\n\
[Parsed_astats_1 @ 0x2] Overall\n\
[Parsed_astats_1 @ 0x2] Number of samples: 720000\n";
        assert_eq!(parse_decoded_seconds(log), Some(15.0));
        assert_eq!(
            parse_decoded_seconds(b"[Parsed_astats_1 @ 0x2] Number of samples: 0\n"),
            None
        );
        assert_eq!(parse_decoded_seconds(b"no statistics"), None);
    }

    #[test]
    fn fade_out_is_placed_from_the_decoded_length_and_records_the_stated_one() {
        let opts = job(r#", "fade": true"#);
        let fade = fade_out_placement(&opts, 15.0, 0.0, Some(5.0065));
        assert_eq!(fade.start_seconds, 14.5);
        assert_eq!(fade.duration_seconds, 0.5);
        assert_eq!(fade.decoded_source_seconds, 15.0);
        assert_eq!(fade.stated_source_seconds, Some(5.0065));
        assert_eq!(
            proc_filters(&opts, None, Some(fade.decoded_source_seconds), None)[0],
            "afade=t=out:st=14.500:d=0.5"
        );
        assert_eq!(fade_out_placement(&opts, 0.2, 0.0, None).start_seconds, 0.0);
    }

    #[test]
    fn ffmpeg_timeout_is_bounded_in_both_directions() {
        assert_eq!(bounded_ffmpeg_timeout(0), MIN_FFMPEG_TIMEOUT_SECS);
        assert_eq!(bounded_ffmpeg_timeout(300), 300);
        assert_eq!(bounded_ffmpeg_timeout(u64::MAX), MAX_FFMPEG_TIMEOUT_SECS);
    }
}
