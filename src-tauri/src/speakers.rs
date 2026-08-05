use std::path::Path;
use std::sync::Arc;

use tauri::AppHandle;

use crate::models;

// ── Active speaker-slot estimation ──────────────────────────────────────────
//
// Uses the pyannote speaker segmentation model to estimate how many speaker
// slots are active. Embedding extraction and cross-window clustering are not
// implemented, so this must not be presented as full distinct-voice analysis.
//
// Pipeline:
//   1. Segment audio into speech windows (speaker_seg_int8.onnx)
//   2. Decode the model's three speaker slots from powerset classes

/// Speaker detection result.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SpeakerInfo {
    /// Estimated number of active segmentation-model slots. This is not a
    /// distinct-voice count because embedding and clustering are not run.
    pub count: u32,
    /// Whether embedding extraction and clustering actually ran.
    pub full_analysis: bool,
}

/// Estimate active segmentation-model speaker slots in an audio file.
pub(crate) async fn detect_speakers(
    app: &AppHandle,
    audio_path: &Path,
    ctx: Option<&crate::analysis::ScanCtx>,
) -> Result<SpeakerInfo, String> {
    crate::safety::check_file_safe(audio_path)?;
    // Check model availability
    let seg_path = models::model_path(app, "speaker_seg_int8.onnx")?;
    let preparation_context = ctx.cloned();
    let preparation_cancelled: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
        preparation_context
            .as_ref()
            .is_some_and(crate::analysis::ScanCtx::cancelled)
    });
    let prepared =
        crate::helpers::prepare_audio_feed_cancellable(audio_path.to_path_buf(), preparation_cancelled).await;
    if ctx.is_some_and(crate::analysis::ScanCtx::cancelled) {
        return Err("Scan cancelled".into());
    }
    let (prepared_audio, _prepared_guard) = prepared?;
    let audio_path = prepared_audio.as_path();

    // Decode to 16kHz mono WAV (drop guard cleans up on every exit path)
    let tmp = crate::safety::TempFile::new(std::env::temp_dir().join(format!(
        "depoaudio_spk_{}.wav",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    )));

    // FTR must use FFmpeg's native multichannel wrapper, never plain AAC.
    let mut args = crate::helpers::safe_ffmpeg_input_prelude();
    let input_codec = crate::helpers::input_codec_args(audio_path);
    if !input_codec.is_empty() {
        crate::ffmpeg::ensure_ftr_decoder(app).await?;
    }
    args.extend(input_codec);
    args.extend([
        "-t".into(),
        "60".into(), // Analyze first 60 seconds only (speed)
        "-i".into(),
        audio_path.to_string_lossy().to_string(),
        "-af".into(),
        "aresample=16000".into(),
        "-ac".into(),
        "1".into(),
        "-acodec".into(),
        "pcm_s16le".into(),
        "-y".into(),
        tmp.to_string_lossy().to_string(),
    ]);

    let output = crate::analysis::sidecar_with_heartbeat(
        app,
        crate::helpers::ffmpeg_bin_name(),
        args,
        60,
        ctx,
        "speakers",
        0.94,
    )
    .await
    .ok_or_else(|| "Failed to decode audio for speaker detection".to_string())?;

    if !output.success {
        return Err("Failed to decode audio for speaker detection".into());
    }

    let samples = crate::types::read_pcm16_mono_wav_bounded(&tmp, 16_000 * 60)?;

    drop(tmp);

    if samples.is_empty() {
        return Ok(SpeakerInfo {
            count: 1,
            full_analysis: false,
        });
    }

    if let Some(c) = ctx {
        c.check()?;
    }

    // Session load + inference on the blocking pool (see scoring.rs).
    tauri::async_runtime::spawn_blocking(move || -> Result<SpeakerInfo, String> {
        let seg_session = models::cached_session(&seg_path)?;
        let mut seg_session = seg_session
            .lock()
            .map_err(|_| "Segmentation session poisoned".to_string())?;

        // Segmentation: pyannote model expects [1, 1, num_samples] input
        // and outputs [1, num_frames, num_speakers] speaker activity probabilities
        let num_samples = samples.len();
        let input = ndarray::Array3::from_shape_vec((1, 1, num_samples), samples)
            .map_err(|e| format!("Tensor error: {}", e))?;
        let input_val = ort::value::Tensor::from_array(input).map_err(|e| format!("Tensor error: {}", e))?;

        let seg_outputs = seg_session
            .run(ort::inputs!["x" => input_val])
            .map_err(|e| format!("Segmentation inference failed: {}", e))?;

        // Parse segmentation output to count active speaker slots
        let first_output = seg_outputs.values().next().ok_or("No segmentation output")?;
        let seg_tensor = first_output
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("Failed to extract segmentation output: {}", e))?;

        let seg_shape = seg_tensor.0;
        // Output is [1, num_frames, 7]: log-probabilities over the pyannote
        // powerset classes {none, S1, S2, S3, S1+S2, S1+S3, S2+S3}. Decode by
        // per-frame argmax, then count speakers with sustained activity.
        let num_classes = if seg_shape.len() == 3 {
            seg_shape[2] as usize
        } else {
            7usize
        };
        let num_frames = if seg_shape.len() == 3 {
            seg_shape[1] as usize
        } else {
            1usize
        };
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

        // A model slot counts as active if present in > 10% of frames.
        let min_activity_ratio = 0.1;
        let active_speakers = active_frames
            .iter()
            .filter(|&&n| n as f64 / num_frames.max(1) as f64 > min_activity_ratio)
            .count() as u32;

        // At least 1 speaker
        let count = active_speakers.max(1);

        Ok(SpeakerInfo {
            count,
            full_analysis: false,
        })
    })
    .await
    .map_err(|e| format!("Speaker detection task failed: {}", e))?
}
