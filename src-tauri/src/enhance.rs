use crate::helpers::resample_linear;
use crate::models;
use crate::types::AudioBuffer;

// ── Bandwidth extension (audio super-resolution) ────────────────────────────
//
// Upscales narrow-band audio (8–16 kHz phone recordings, old equipment)
// to 48 kHz. Uses FlashSR ONNX model when available, falls back to
// FFmpeg SoX high-quality resampler.

/// Bandwidth extension on an AudioBuffer using FlashSR.
/// FlashSR expects 16kHz mono input and produces 48kHz output, so each
/// channel is processed independently. Channels are never downmixed: in
/// multi-channel court recordings each channel is a separate speaker mic,
/// and mixing them would destroy per-speaker separation.
/// If the model is not available, this is a no-op (returns Ok).
pub(crate) fn enhance_buffer(
    app: &tauri::AppHandle,
    buf: &mut AudioBuffer,
) -> Result<(), String> {
    let model_path = match models::model_path(app, "flashsr.onnx") {
        Ok(p) => p,
        Err(_) => return Ok(()), // No model available — no-op
    };
    let mut session = models::load_session(&model_path)?;

    let original_rate = buf.sample_rate;
    let channel_bufs = buf.channels_split();
    let mut processed = Vec::with_capacity(channel_bufs.len());
    for ch_samples in &channel_bufs {
        processed.push(enhance_channel(&mut session, ch_samples, original_rate)?);
    }
    *buf = AudioBuffer::from_channels(&processed, 48000);

    Ok(())
}

/// Run one mono channel through FlashSR, returning 48kHz samples.
fn enhance_channel(
    session: &mut ort::session::Session,
    samples: &[f32],
    original_rate: u32,
) -> Result<Vec<f32>, String> {
    let samples_16k = resample_linear(samples, original_rate, 16000);

    if samples_16k.is_empty() {
        return Err("Empty audio for FlashSR".into());
    }

    let input_len = samples_16k.len();
    let input_tensor = ndarray::Array2::from_shape_vec((1, input_len), samples_16k)
        .map_err(|e| format!("Tensor error: {}", e))?;
    let input_val = ort::value::Tensor::from_array(input_tensor)
        .map_err(|e| format!("Tensor error: {}", e))?;

    let outputs = session
        .run(ort::inputs!["input" => input_val])
        .map_err(|e| format!("FlashSR inference failed: {}", e))?;

    let first_output = outputs
        .values()
        .next()
        .ok_or("No FlashSR output")?;
    let output_tensor = first_output
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("Failed to extract FlashSR output: {}", e))?;

    Ok(output_tensor.1.to_vec())
}
