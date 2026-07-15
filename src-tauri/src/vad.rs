use std::path::Path;

use tauri::AppHandle;

use crate::models;

// ── Voice Activity Detection (Silero VAD) ───────────────────────────────────
//
// Detects speech vs. silence at 10ms granularity. Used to:
//   1. Skip silent regions during denoising (faster processing)
//   2. Measure loudness only during speech (better auto-leveling)
//   3. Pair with Smart Turn for more accurate turn boundaries
//
// Silero VAD operates on 16kHz mono audio in 512-sample chunks (~32ms).
// Output is a probability [0.0, 1.0] where > 0.5 indicates speech.

/// A detected speech segment with start/end times.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SpeechSegment {
    pub start: f64,
    pub end: f64,
    /// Average VAD probability across the segment.
    pub confidence: f64,
}

/// Why VAD produced no result — the distinction matters: an undecodable FILE
/// should short-circuit the other analysis passes (they'd burn their decode
/// timeouts on the same bytes), while an unavailable VAD ENVIRONMENT (model
/// missing/corrupt, ORT failure) says nothing about the file and must not
/// skip passes whose own models are fine.
#[derive(Debug)]
pub(crate) enum VadError {
    /// FFmpeg could not decode this file (or the decode timed out).
    Undecodable(String),
    /// VAD itself is unavailable/failed; the file may be perfectly fine.
    Unavailable(String),
}

impl std::fmt::Display for VadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VadError::Undecodable(e) | VadError::Unavailable(e) => write!(f, "{}", e),
        }
    }
}

/// VAD analysis result.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VadResult {
    /// Detected speech segments.
    pub segments: Vec<SpeechSegment>,
    /// Total speech duration in seconds.
    pub speech_duration: f64,
    /// Total silence duration in seconds.
    pub silence_duration: f64,
    /// Speech-to-total ratio (0.0 - 1.0).
    pub speech_ratio: f64,
}

/// Run voice activity detection on an audio file.
/// Returns speech segments with timestamps.
pub(crate) async fn detect_speech(
    app: &AppHandle,
    audio_path: &Path,
    ctx: Option<&crate::analysis::ScanCtx>,
) -> Result<VadResult, VadError> {
    let model_path = models::model_path(app, "silero_vad.onnx").map_err(VadError::Unavailable)?;

    // Decode to 16kHz mono WAV (drop guard cleans up on every exit path)
    let tmp = crate::safety::TempFile::new(std::env::temp_dir().join(format!(
        "depoaudio_vad_{}.wav",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    )));

    // Forced input decoder for formats ffmpeg can't auto-detect (FTR)
    let mut args: Vec<String> = crate::helpers::scan_input_codec_args(audio_path);
    args.extend([
        "-t".into(), crate::ffmpeg::ANALYSIS_SAMPLE_SECS.to_string(),
        "-i".into(), audio_path.to_string_lossy().to_string(),
        "-af".into(), "aresample=16000".into(),
        "-ac".into(), "1".into(),
        "-acodec".into(), "pcm_s16le".into(),
        "-y".into(), tmp.to_string_lossy().to_string(),
    ]);

    // Bounded decode (sample cap + timeout) so VAD can't hang the scan.
    // Decode failures are Undecodable — the file itself is the problem.
    let output = crate::analysis::sidecar_with_heartbeat(
        app, crate::helpers::ffmpeg_bin_name(), args, 120, ctx, "speech", 0.33,
    )
    .await
    .ok_or_else(|| VadError::Undecodable("Failed to decode audio for VAD".into()))?;

    if !output.success {
        return Err(VadError::Undecodable("Failed to decode audio for VAD".into()));
    }

    let reader = hound::WavReader::open(&tmp)
        .map_err(|e| VadError::Unavailable(format!("WAV read error: {}", e)))?;
    // Read as i16 and normalize to [-1.0, 1.0] float range for Silero VAD
    let samples: Vec<f32> = reader
        .into_samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / 32768.0)
        .collect();

    drop(tmp);

    if samples.is_empty() {
        return Ok(VadResult {
            segments: vec![],
            speech_duration: 0.0,
            silence_duration: 0.0,
            speech_ratio: 0.0,
        });
    }

    // Inference runs on the blocking pool: ~5,600 sequential session.run()
    // calls would otherwise pin an async worker thread (and with several
    // in-flight scans, starve tokio's timers app-wide).
    let samples_len = samples.len();
    let ctx_owned = ctx.cloned();
    let probabilities = tauri::async_runtime::spawn_blocking(move || -> Result<Vec<f32>, String> {
        let mut session = models::load_session(&model_path)?;

        // Silero VAD v5 processes 512-sample chunks at 16kHz (~32ms each)
        // Inputs: "input" [1, chunk_size], "state" [2, 1, 128], "sr" scalar i64
        // Outputs: "output" [1, 1], "stateN" [2, 1, 128]
        let chunk_size = 512usize;
        let sample_rate = 16000usize;
        let num_chunks = samples.len() / chunk_size;

        // Silero VAD v5 uses a single state tensor [2, 1, 128]
        let mut state = ndarray::Array3::<f32>::zeros((2, 1, 128));
        let sr = ndarray::Array1::from_vec(vec![sample_rate as i64]);

        let mut probabilities: Vec<f32> = Vec::with_capacity(num_chunks);

        for i in 0..num_chunks {
            // A cancelled scan must stop computing, not just stop reporting —
            // and a slow-but-healthy loop must keep the frontend's stall
            // watchdog fed (up to ~5,600 chunks can take minutes on weak CPUs)
            if i % 256 == 0 {
                if let Some(c) = &ctx_owned {
                    if c.cancelled() { return Err("Scan cancelled".into()); }
                    c.emit("speech", 0.34 + 0.10 * (i as f64 / num_chunks.max(1) as f64));
                }
            }

            let start = i * chunk_size;
            let chunk: Vec<f32> = samples[start..start + chunk_size].to_vec();
            let input = ndarray::Array2::from_shape_vec((1, chunk_size), chunk)
                .map_err(|e| format!("Tensor error: {}", e))?;

            let input_val = ort::value::Tensor::from_array(input)
                .map_err(|e| format!("Tensor error: {}", e))?;
            let sr_val = ort::value::Tensor::from_array(sr.clone())
                .map_err(|e| format!("Tensor error: {}", e))?;
            let state_val = ort::value::Tensor::from_array(state.clone())
                .map_err(|e| format!("Tensor error: {}", e))?;
            let result = session.run(ort::inputs![
                "input" => input_val,
                "state" => state_val,
                "sr" => sr_val
            ]);

            match result {
                Ok(outputs) => {
                    // Extract probability
                    if let Some(prob_val) = outputs.get("output") {
                        if let Ok(prob_tensor) = prob_val.try_extract_tensor::<f32>() {
                            let prob = prob_tensor.1.first().copied().unwrap_or(0.0);
                            probabilities.push(prob);
                        }
                    }

                    // Update state for next chunk
                    if let Some(state_out) = outputs.get("stateN") {
                        if let Ok(state_tensor) = state_out.try_extract_tensor::<f32>() {
                            let slice: &[f32] = state_tensor.1;
                            if slice.len() == state.len() {
                                state.as_slice_mut().unwrap().copy_from_slice(slice);
                            }
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[vad] inference error at chunk {}: {}", i, e);
                    probabilities.push(0.0);
                }
            }
        }

        Ok(probabilities)
    })
    .await
    .map_err(|e| VadError::Unavailable(format!("VAD task failed: {}", e)))?
    .map_err(VadError::Unavailable)?;

    let chunk_size = 512usize;
    let sample_rate = 16000usize;
    let num_chunks = probabilities.len();
    let threshold = 0.5f32;

    // Convert frame-level probabilities into speech segments
    let chunk_duration = chunk_size as f64 / sample_rate as f64;
    let mut segments: Vec<SpeechSegment> = Vec::new();
    let mut in_speech = false;
    let mut seg_start = 0.0;
    let mut seg_probs: Vec<f32> = Vec::new();

    for (i, &prob) in probabilities.iter().enumerate() {
        let time = i as f64 * chunk_duration;

        if prob > threshold && !in_speech {
            // Speech started
            in_speech = true;
            seg_start = time;
            seg_probs.clear();
            seg_probs.push(prob);
        } else if prob > threshold && in_speech {
            seg_probs.push(prob);
        } else if prob <= threshold && in_speech {
            // Speech ended
            in_speech = false;
            let avg_prob = seg_probs.iter().sum::<f32>() / seg_probs.len() as f32;
            segments.push(SpeechSegment {
                start: seg_start,
                end: time,
                confidence: avg_prob as f64,
            });
        }
    }

    // Close any open segment
    if in_speech {
        let avg_prob = seg_probs.iter().sum::<f32>() / seg_probs.len().max(1) as f32;
        segments.push(SpeechSegment {
            start: seg_start,
            end: num_chunks as f64 * chunk_duration,
            confidence: avg_prob as f64,
        });
    }

    // Merge segments separated by < 300ms (typical pause tolerance)
    let merge_gap = 0.3;
    let mut merged: Vec<SpeechSegment> = Vec::new();
    for seg in segments {
        if let Some(last) = merged.last_mut() {
            if seg.start - last.end < merge_gap {
                last.end = seg.end;
                last.confidence = (last.confidence + seg.confidence) / 2.0;
                continue;
            }
        }
        merged.push(seg);
    }

    // Calculate stats
    let total_duration = samples_len as f64 / sample_rate as f64;
    let speech_duration: f64 = merged.iter().map(|s| s.end - s.start).sum();
    let silence_duration = total_duration - speech_duration;
    let speech_ratio = if total_duration > 0.0 { speech_duration / total_duration } else { 0.0 };

    Ok(VadResult {
        segments: merged,
        speech_duration,
        silence_duration,
        speech_ratio,
    })
}
