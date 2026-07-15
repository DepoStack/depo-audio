use std::path::Path;

use tauri::AppHandle;

use crate::models;

// ── Speaker count detection ─────────────────────────────────────────────────
//
// Uses pyannote speaker segmentation + speaker embedding models to detect
// how many distinct speakers are in the recording. Helps auto-configure
// channel labels and identify which channels are active.
//
// Pipeline:
//   1. Segment audio into speech windows (speaker_seg_int8.onnx)
//   2. Extract embeddings per segment (speaker_embed.onnx)
//   3. Cluster embeddings to count unique speakers

/// Speaker detection result.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerInfo {
    /// Estimated number of distinct speakers.
    pub count: u32,
    /// Whether the speaker embedding model was available for full analysis.
    pub full_analysis: bool,
}

/// Detect number of speakers in an audio file.
pub(crate) async fn detect_speakers(
    app: &AppHandle,
    audio_path: &Path,
    ctx: Option<&crate::analysis::ScanCtx>,
) -> Result<SpeakerInfo, String> {
    // Check model availability
    let seg_path = models::model_path(app, "speaker_seg_int8.onnx")?;

    let has_embed = models::model_path(app, "speaker_embed.onnx").is_ok();

    // Decode to 16kHz mono WAV (drop guard cleans up on every exit path)
    let tmp = crate::safety::TempFile::new(std::env::temp_dir().join(format!(
        "depoaudio_spk_{}.wav",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    )));

    // Forced input decoder for formats ffmpeg can't auto-detect (FTR)
    let mut args: Vec<String> = crate::helpers::scan_input_codec_args(audio_path);
    args.extend([
        "-t".into(), "60".into(), // Analyze first 60 seconds only (speed)
        "-i".into(), audio_path.to_string_lossy().to_string(),
        "-af".into(), "aresample=16000".into(),
        "-ac".into(), "1".into(),
        "-acodec".into(), "pcm_s16le".into(),
        "-y".into(), tmp.to_string_lossy().to_string(),
    ]);

    let output = crate::ffmpeg::sidecar_output_opt(app, crate::helpers::ffmpeg_bin_name(), args, 60)
        .await
        .ok_or_else(|| "Failed to decode audio for speaker detection".to_string())?;

    if !output.success {
        return Err("Failed to decode audio for speaker detection".into());
    }

    let reader = hound::WavReader::open(&tmp)
        .map_err(|e| format!("Failed to open WAV: {}", e))?;

    let samples: Vec<f32> = reader
        .into_samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / 32768.0)
        .collect();

    drop(tmp);

    if samples.is_empty() {
        return Ok(SpeakerInfo { count: 1, full_analysis: false });
    }

    // The 60s -t cap limits by input timestamps; corrupt timestamps can yield
    // far more decoded samples. Hard-cap the tensor so a single inference
    // can't be handed an arbitrarily large recurrent input.
    let max_samples = 16000 * 60;
    let samples = if samples.len() > max_samples { samples[..max_samples].to_vec() } else { samples };

    if let Some(c) = ctx { c.check()?; }

    // Session load + inference on the blocking pool (see scoring.rs).
    tauri::async_runtime::spawn_blocking(move || -> Result<SpeakerInfo, String> {
        let mut seg_session = models::load_session(&seg_path)?;

        // Segmentation: pyannote model expects [1, 1, num_samples] input
        // and outputs [1, num_frames, num_speakers] speaker activity probabilities
        let num_samples = samples.len();
        let input = ndarray::Array3::from_shape_vec(
            (1, 1, num_samples),
            samples,
        ).map_err(|e| format!("Tensor error: {}", e))?;
        let input_val = ort::value::Tensor::from_array(input)
            .map_err(|e| format!("Tensor error: {}", e))?;

        let seg_outputs = seg_session
            .run(ort::inputs!["x" => input_val])
            .map_err(|e| format!("Segmentation inference failed: {}", e))?;

        // Parse segmentation output to count active speaker slots
        let first_output = seg_outputs
            .values()
            .next()
            .ok_or("No segmentation output")?;
        let seg_tensor = first_output
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to extract segmentation output: {}", e))?;

        let seg_shape = seg_tensor.0;
        // Output is [1, num_frames, 7]: log-probabilities over the pyannote
        // powerset classes {none, S1, S2, S3, S1+S2, S1+S3, S2+S3}. Decode by
        // per-frame argmax, then count speakers with sustained activity.
        let num_classes = if seg_shape.len() == 3 { seg_shape[2] as usize } else { 7usize };
        let num_frames = if seg_shape.len() == 3 { seg_shape[1] as usize } else { 1usize };
        let seg_data = seg_tensor.1;

        const POWERSET: [&[usize]; 7] = [&[], &[0], &[1], &[2], &[0, 1], &[0, 2], &[1, 2]];
        let mut active_frames = [0usize; 3];

        for f in 0..num_frames {
            let mut best_class = 0usize;
            let mut best_val = f32::NEG_INFINITY;
            for k in 0..num_classes.min(POWERSET.len()) {
                let idx = f * num_classes + k;
                if idx < seg_data.len() && seg_data[idx] > best_val {
                    best_val = seg_data[idx];
                    best_class = k;
                }
            }
            for &s in POWERSET[best_class] {
                active_frames[s] += 1;
            }
        }

        // A speaker counts if active in > 10% of frames
        let min_activity_ratio = 0.1;
        let active_speakers = active_frames
            .iter()
            .filter(|&&n| n as f64 / num_frames.max(1) as f64 > min_activity_ratio)
            .count() as u32;

        // At least 1 speaker
        let count = active_speakers.max(1);

        Ok(SpeakerInfo {
            count,
            full_analysis: has_embed, // Full embedding analysis would refine this further
        })
    })
    .await
    .map_err(|e| format!("Speaker detection task failed: {}", e))?
}
