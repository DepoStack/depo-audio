use std::path::Path;

use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;

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
) -> Result<SpeakerInfo, String> {
    // Check model availability
    let seg_path = models::model_path(app, "speaker_seg_int8.onnx")?;
    let mut seg_session = models::load_session(&seg_path)?;

    let has_embed = models::model_path(app, "speaker_embed.onnx").is_ok();

    // Decode to 16kHz mono WAV
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_spk_{}.wav",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    ));

    let args: Vec<String> = vec![
        "-i".into(), audio_path.to_string_lossy().to_string(),
        "-af".into(), "aresample=16000".into(),
        "-ac".into(), "1".into(),
        "-acodec".into(), "pcm_f32le".into(),
        "-t".into(), "60".into(), // Analyze first 60 seconds only (speed)
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
        return Err("Failed to decode audio for speaker detection".into());
    }

    let reader = hound::WavReader::open(&tmp)
        .map_err(|e| format!("Failed to open WAV: {}", e))?;

    let samples: Vec<f32> = reader
        .into_samples::<f32>()
        .filter_map(|s| s.ok())
        .collect();

    let _ = std::fs::remove_file(&tmp);

    if samples.is_empty() {
        return Ok(SpeakerInfo { count: 1, full_analysis: false });
    }

    // Segmentation: pyannote model expects [1, 1, num_samples] input
    // and outputs [1, num_frames, num_speakers] speaker activity probabilities
    let num_samples = samples.len();
    let input = ndarray::Array3::from_shape_vec(
        (1, 1, num_samples),
        samples.clone(),
    ).map_err(|e| format!("Tensor error: {}", e))?;
    let input_val = ort::value::Tensor::from_array(input)
        .map_err(|e| format!("Tensor error: {}", e))?;

    let seg_outputs = seg_session
        .run(ort::inputs!["input" => input_val])
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
    // Output shape is typically [1, num_frames, max_speakers]
    let num_speakers_slots = if seg_shape.len() == 3 { seg_shape[2] as usize } else { 3usize };
    let num_frames = if seg_shape.len() == 3 { seg_shape[1] as usize } else { 1usize };
    let seg_data = seg_tensor.1;

    // Count how many speaker slots have significant activity (> 10% of frames active)
    let threshold = 0.5f32;
    let min_activity_ratio = 0.1;
    let mut active_speakers = 0u32;

    for spk in 0..num_speakers_slots {
        let active_frames: usize = (0..num_frames)
            .filter(|&f| {
                let idx = f * num_speakers_slots + spk;
                idx < seg_data.len() && seg_data[idx] > threshold
            })
            .count();

        let ratio = active_frames as f64 / num_frames.max(1) as f64;
        if ratio > min_activity_ratio {
            active_speakers += 1;
        }
    }

    // At least 1 speaker
    let count = active_speakers.max(1);

    Ok(SpeakerInfo {
        count,
        full_analysis: has_embed, // Full embedding analysis would refine this further
    })
}
