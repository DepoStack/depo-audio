use std::path::Path;

use regex::Regex;
use tauri::AppHandle;

use crate::ffmpeg::{probe_channels, probe_duration};
use crate::helpers::ffprobe_bin_name;
use crate::types::{AnalysisResult, TurnSegment};

// Used by detect_turns for ONNX inference
use hound;
use ndarray;

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

/// Run full audio analysis on a file.
pub(crate) async fn analyze_audio(
    app: &AppHandle,
    path: &str,
) -> Result<AnalysisResult, String> {
    let feed = Path::new(path);
    crate::safety::check_file_safe(feed)?;

    // Probe basic metadata. A wrong fallback here is tolerable: per-channel
    // stats for nonexistent channels read as silence and get filtered out.
    let channels = probe_channels(app, feed).await.unwrap_or(4);
    let duration = probe_duration(app, feed).await.unwrap_or(0.0);
    let sample_rate = probe_sample_rate(app, feed).await.unwrap_or(48000);

    // Run loudness + peak analysis per channel
    let (per_channel_lufs, per_channel_peak) =
        analyze_loudness_and_peaks(app, feed, channels).await?;

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
    let needs_denoise = estimate_noise_floor(app, feed).await > NOISE_THRESHOLD;

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
    let vad_result = crate::vad::detect_speech(app, std::path::Path::new(path)).await.ok();
    let speech_ratio = vad_result.as_ref().map(|v| v.speech_ratio).unwrap_or(1.0);

    // Smart Turn detection — skip if very little speech detected
    let turns = if speech_ratio > 0.1 {
        detect_turns(app, feed, channels, duration).await
    } else {
        Vec::new()
    };

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
    let quality_score = if speech_ratio > 0.05 {
        crate::scoring::score_quality(app, std::path::Path::new(path)).await
            .map(|qs| crate::types::QualityScoreResult { sig: qs.sig, bak: qs.bak, ovr: qs.ovr })
            .ok()
    } else {
        None
    };

    // Speaker count detection — skip if no speech detected
    let speaker_count = if speech_ratio > 0.1 {
        crate::speakers::detect_speakers(app, std::path::Path::new(path)).await
            .map(|info| info.count)
            .ok()
    } else {
        None
    };

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
) -> Result<(Vec<f64>, Vec<f64>), String> {
    let mut lufs_vec = Vec::with_capacity(channels as usize);
    let mut peak_vec = Vec::with_capacity(channels as usize);

    if channels <= 1 {
        // Mono or single-channel: analyze directly
        let (lufs, peak) = analyze_single_channel(app, feed, None).await?;
        lufs_vec.push(lufs);
        peak_vec.push(peak);
    } else {
        // Multi-channel: use channelsplit + per-channel ebur128
        for ch in 0..channels {
            let (lufs, peak) = analyze_single_channel(app, feed, Some(ch)).await?;
            lufs_vec.push(lufs);
            peak_vec.push(peak);
        }
    }

    Ok((lufs_vec, peak_vec))
}

async fn analyze_single_channel(
    app: &AppHandle,
    feed: &Path,
    channel: Option<u32>,
) -> Result<(f64, f64), String> {
    let feed_str = feed.to_string_lossy().to_string();

    // Build filter: optionally extract a single channel via pan, then run ebur128.
    // Using pan=mono instead of channelsplit avoids hardcoding a channel layout.
    // `-t` (input option, before `-i`) limits analysis to a representative
    // sample so a long recording can't make this pass run for minutes.
    let secs = crate::ffmpeg::ANALYSIS_SAMPLE_SECS.to_string();
    let args: Vec<String> = if let Some(ch) = channel {
        let pan = format!("pan=mono|c0=c{}", ch);
        let filter = format!("{},ebur128=peak=true", pan);
        vec![
            "-t".into(), secs,
            "-i".into(), feed_str,
            "-af".into(), filter,
            "-f".into(), "null".into(), "-".into(),
        ]
    } else {
        vec![
            "-t".into(), secs,
            "-i".into(), feed_str,
            "-af".into(), "ebur128=peak=true".into(),
            "-f".into(), "null".into(), "-".into(),
        ]
    };

    // Bounded timeout backstop — the -t cap means a healthy run finishes in
    // seconds, so a wedged ffmpeg can never hang the Scan.
    let output = crate::ffmpeg::sidecar_output_opt(app, crate::helpers::ffmpeg_bin_name(), args, 120)
        .await
        .ok_or_else(|| "Loudness analysis timed out".to_string())?;

    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    // Parse integrated loudness: "I: -XX.X LUFS"
    let lufs_re = Regex::new(r"I:\s+(-?\d+\.?\d*)\s+LUFS").unwrap();
    let lufs = lufs_re
        .captures_iter(&stderr)
        .last()
        .and_then(|c| c[1].parse::<f64>().ok())
        .unwrap_or(-70.0);

    // Parse true peak: "Peak: -XX.X dBFS"
    let peak_re = Regex::new(r"Peak:\s+(-?\d+\.?\d*)\s+dBFS").unwrap();
    let peak = peak_re
        .captures_iter(&stderr)
        .last()
        .and_then(|c| c[1].parse::<f64>().ok())
        .unwrap_or(-70.0);

    Ok((lufs, peak))
}

// ── Noise floor estimation ──────────────────────────────────────────────────

async fn estimate_noise_floor(app: &AppHandle, feed: &Path) -> f64 {
    // Use astats' measured noise floor. The overall RMS level would include
    // speech and sits far above any sensible noise threshold, which made
    // denoising look "recommended" for virtually every normal recording.
    let args: Vec<String> = vec![
        "-t".into(), crate::ffmpeg::ANALYSIS_SAMPLE_SECS.to_string(),
        "-i".into(), feed.to_string_lossy().to_string(),
        "-af".into(), "astats=metadata=1".into(),
        "-f".into(), "null".into(), "-".into(),
    ];

    if let Some(out) = crate::ffmpeg::sidecar_output_opt(app, crate::helpers::ffmpeg_bin_name(), args, 120).await {
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
    _duration: f64,
) -> Vec<TurnSegment> {
    // Try loading the Smart Turn model — if not available, return empty
    let model_path = match crate::models::model_path(app, "smart-turn-v3-int8.onnx") {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let mut session = match crate::models::load_session(&model_path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    // Decode to 16kHz mono WAV per channel for the model
    let mut all_turns = Vec::new();

    for ch in 0..channels {
        let tmp = std::env::temp_dir().join(format!("depoaudio_turn_ch{}_{}.wav", ch, uuid::Uuid::new_v4()));
        let pan_filter = if channels > 1 {
            format!("pan=mono|c0=c{},aresample=16000", ch)
        } else {
            "aresample=16000".into()
        };

        let args: Vec<String> = vec![
            "-t".into(), crate::ffmpeg::ANALYSIS_SAMPLE_SECS.to_string(),
            "-i".into(), feed.to_string_lossy().to_string(),
            "-af".into(), pan_filter,
            "-acodec".into(), "pcm_s16le".into(),
            "-y".into(), tmp.to_string_lossy().to_string(),
        ];

        let ok = crate::ffmpeg::sidecar_output_opt(app, crate::helpers::ffmpeg_bin_name(), args, 120)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !ok || !tmp.exists() { continue; }

        // Read the decoded WAV and run turn detection
        if let Ok(reader) = hound::WavReader::open(&tmp) {
            let samples: Vec<f32> = reader
                .into_samples::<i16>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / 32768.0)
                .collect();

            let sample_rate = 16000usize;
            let window_size = sample_rate * 8; // 8 seconds
            let stride = sample_rate; // 1 second stride
            let mut pos = 0usize;
            let mut turn_start: Option<f64> = None;

            while pos + window_size <= samples.len() {
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
            }
        }

        let _ = std::fs::remove_file(&tmp);
    }

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
