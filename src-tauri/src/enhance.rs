use std::path::{Path, PathBuf};

use tauri_plugin_shell::ShellExt;
use uuid::Uuid;

use crate::models;

// ── Bandwidth extension (audio super-resolution) ────────────────────────────
//
// Upscales narrow-band audio (8–16 kHz phone recordings, old equipment)
// to 48 kHz. Uses FlashSR ONNX model when available, falls back to
// FFmpeg SoX high-quality resampler.

/// Upscale narrow-band audio to 48 kHz.
/// Tries FlashSR neural upscaling first, falls back to SoX resampler.
pub(crate) async fn enhance_bandwidth(
    app: &tauri::AppHandle,
    input: &Path,
) -> Result<PathBuf, String> {
    // Try neural upscaling with FlashSR if the model is available
    if let Ok(model_path) = models::model_path(app, "flashsr.onnx") {
        if let Ok(result) = enhance_with_flashsr(app, input, &model_path).await {
            return Ok(result);
        }
        // Fall through to SoX resampler on failure
    }

    // Fallback: FFmpeg SoX high-quality resampler
    enhance_with_soxr(app, input).await
}

/// Neural bandwidth extension using FlashSR ONNX model.
/// FlashSR expects 16kHz mono input and produces 48kHz output.
async fn enhance_with_flashsr(
    app: &tauri::AppHandle,
    input: &Path,
    model_path: &PathBuf,
) -> Result<PathBuf, String> {
    let session = models::load_session(model_path)?;

    // Decode input to 16kHz mono f32 WAV
    let tmp_in = std::env::temp_dir().join(format!(
        "depoaudio_fsr_in_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let args: Vec<String> = vec![
        "-i".into(), input.to_string_lossy().to_string(),
        "-af".into(), "aresample=16000".into(),
        "-ac".into(), "1".into(),
        "-acodec".into(), "pcm_f32le".into(),
        "-y".into(), tmp_in.to_string_lossy().to_string(),
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
        let _ = std::fs::remove_file(&tmp_in);
        return Err("Failed to decode for FlashSR".into());
    }

    let reader = hound::WavReader::open(&tmp_in)
        .map_err(|e| format!("WAV read error: {}", e))?;
    let samples: Vec<f32> = reader
        .into_samples::<f32>()
        .filter_map(|s| s.ok())
        .collect();

    let _ = std::fs::remove_file(&tmp_in);

    if samples.is_empty() {
        return Err("Empty audio for FlashSR".into());
    }

    // FlashSR processes in chunks. Process the entire signal.
    let input_len = samples.len();
    let input_tensor = ndarray::Array2::from_shape_vec((1, input_len), samples)
        .map_err(|e| format!("Tensor error: {}", e))?;

    let outputs = session
        .run(ort::inputs!["input" => input_tensor.view()])
        .map_err(|e| format!("FlashSR inference failed: {}", e))?;

    let output_tensor = outputs
        .values()
        .next()
        .and_then(|v| v.try_extract_tensor::<f32>().ok())
        .ok_or("Failed to extract FlashSR output")?;

    let output_samples: Vec<f32> = output_tensor
        .as_slice()
        .ok_or("Cannot read FlashSR output")?
        .to_vec();

    // Write 48kHz output WAV
    let tmp_out = std::env::temp_dir().join(format!(
        "depoaudio_enhanced_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 48000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(&tmp_out, spec)
        .map_err(|e| format!("WAV write error: {}", e))?;

    for &s in &output_samples {
        writer.write_sample(s).map_err(|e| format!("Write error: {}", e))?;
    }
    writer.finalize().map_err(|e| format!("Finalize error: {}", e))?;

    Ok(tmp_out)
}

/// Fallback: high-quality resampling via FFmpeg SoX resampler.
async fn enhance_with_soxr(
    app: &tauri::AppHandle,
    input: &Path,
) -> Result<PathBuf, String> {
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_enhanced_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let args: Vec<String> = vec![
        "-i".into(),
        input.to_string_lossy().to_string(),
        "-af".into(),
        "aresample=resampler=soxr:precision=28:out_sample_rate=48000".into(),
        "-acodec".into(),
        "pcm_f32le".into(),
        "-y".into(),
        tmp.to_string_lossy().to_string(),
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
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Bandwidth enhancement failed: {}",
            stderr.chars().take(200).collect::<String>()
        ));
    }

    Ok(tmp)
}
