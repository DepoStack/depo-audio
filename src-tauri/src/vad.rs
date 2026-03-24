use std::path::Path;

use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;

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
) -> Result<VadResult, String> {
    let model_path = models::model_path(app, "silero_vad.onnx")?;
    let session = models::load_session(&model_path)?;

    // Decode to 16kHz mono WAV
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_vad_{}.wav",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    ));

    let args: Vec<String> = vec![
        "-i".into(), audio_path.to_string_lossy().to_string(),
        "-af".into(), "aresample=16000".into(),
        "-ac".into(), "1".into(),
        "-acodec".into(), "pcm_f32le".into(),
        "-y".into(), tmp.to_string_lossy().to_string(),
    ];

    let output = app
        .shell()
        .sidecar(crate::helpers::ffmpeg_bin_name())
        .map_err(|e| e.to_string())?
        .args(args)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if output.status.code() != Some(0) {
        return Err("Failed to decode audio for VAD".into());
    }

    let reader = hound::WavReader::open(&tmp)
        .map_err(|e| format!("WAV read error: {}", e))?;
    let samples: Vec<f32> = reader
        .into_samples::<f32>()
        .filter_map(|s| s.ok())
        .collect();

    let _ = std::fs::remove_file(&tmp);

    if samples.is_empty() {
        return Ok(VadResult {
            segments: vec![],
            speech_duration: 0.0,
            silence_duration: 0.0,
            speech_ratio: 0.0,
        });
    }

    // Silero VAD processes 512-sample chunks at 16kHz (~32ms each)
    let chunk_size = 512usize;
    let sample_rate = 16000usize;
    let num_chunks = samples.len() / chunk_size;
    let threshold = 0.5f32;

    // Track VAD state for stateful inference
    let mut h = ndarray::Array2::<f32>::zeros((2, 64)); // hidden state
    let mut c = ndarray::Array2::<f32>::zeros((2, 64)); // cell state
    let sr = ndarray::Array1::from_vec(vec![sample_rate as i64]);

    let mut probabilities: Vec<f32> = Vec::with_capacity(num_chunks);

    for i in 0..num_chunks {
        let start = i * chunk_size;
        let chunk: Vec<f32> = samples[start..start + chunk_size].to_vec();
        let input = ndarray::Array2::from_shape_vec((1, chunk_size), chunk)
            .map_err(|e| format!("Tensor error: {}", e))?;

        // Run inference with state
        let result = session.run(ort::inputs![
            "input" => input.view(),
            "sr" => sr.view(),
            "h" => h.view(),
            "c" => c.view()
        ]);

        match result {
            Ok(outputs) => {
                // Extract probability
                if let Some(prob_val) = outputs.get("output") {
                    if let Ok(prob_tensor) = prob_val.try_extract_tensor::<f32>() {
                        let prob = prob_tensor.as_slice()
                            .and_then(|s| s.first().copied())
                            .unwrap_or(0.0);
                        probabilities.push(prob);
                    }
                }

                // Update hidden states for next chunk
                if let Some(hn) = outputs.get("hn") {
                    if let Ok(hn_tensor) = hn.try_extract_tensor::<f32>() {
                        if let Some(slice) = hn_tensor.as_slice() {
                            if slice.len() == h.len() {
                                h.as_slice_mut().unwrap().copy_from_slice(slice);
                            }
                        }
                    }
                }
                if let Some(cn) = outputs.get("cn") {
                    if let Ok(cn_tensor) = cn.try_extract_tensor::<f32>() {
                        if let Some(slice) = cn_tensor.as_slice() {
                            if slice.len() == c.len() {
                                c.as_slice_mut().unwrap().copy_from_slice(slice);
                            }
                        }
                    }
                }
            }
            Err(_) => {
                probabilities.push(0.0);
            }
        }
    }

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
    let total_duration = samples.len() as f64 / sample_rate as f64;
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
