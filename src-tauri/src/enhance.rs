use crate::ffmpeg::CancelCheck;
use crate::helpers::resample_linear;
use crate::models;
use crate::types::{AudioBuffer, CONVERSION_CANCELLED_MESSAGE};

/// The upstream streaming implementation recommends at most one second of
/// 16 kHz input for its highest-quality bounded-latency mode. Keeping each ORT
/// call to this size lets conversion observe cancellation between inferences.
const FLASHSR_FRAME_SAMPLES: usize = 16_000;
const FLASHSR_MIN_FRAME_SAMPLES: usize = 1_000;
const FLASHSR_OVERLAP_INPUT_SAMPLES: usize = 500;
const FLASHSR_UPSAMPLE_FACTOR: usize = 3;

// ── Bandwidth extension (audio super-resolution) ────────────────────────────
//
// Upscales narrow-band audio (8–16 kHz phone recordings, old equipment)
// to 48 kHz using the FlashSR ONNX model.

/// Bandwidth extension on an AudioBuffer using FlashSR.
/// FlashSR expects 16kHz mono input and produces 48kHz output, so each
/// channel is processed independently. Channels are never downmixed: in
/// multi-channel court recordings each channel is a separate speaker mic,
/// and mixing them would destroy per-speaker separation.
pub(crate) fn enhance_buffer(
    app: &tauri::AppHandle,
    buf: &mut AudioBuffer,
    cancelled: CancelCheck<'_>,
) -> Result<(), String> {
    check_cancelled(cancelled)?;
    // A requested enhancement must either run or fail clearly. Silently
    // returning the unmodified signal made successful conversions claim an
    // effect that was never applied.
    let model_path = models::model_path(app, "flashsr.onnx")?;
    let mut session = models::load_session(&model_path)?;

    let original_rate = buf.sample_rate;
    let channel_bufs = buf.channels_split();
    let mut processed = Vec::with_capacity(channel_bufs.len());
    for ch_samples in &channel_bufs {
        check_cancelled(cancelled)?;
        processed.push(enhance_channel(&mut session, ch_samples, original_rate, cancelled)?);
    }
    check_cancelled(cancelled)?;
    *buf = AudioBuffer::from_channels(&processed, 48000);

    Ok(())
}

fn check_cancelled(cancelled: CancelCheck<'_>) -> Result<(), String> {
    if cancelled.map(|check| check()).unwrap_or(false) {
        Err(CONVERSION_CANCELLED_MESSAGE.into())
    } else {
        Ok(())
    }
}

/// Run one mono channel through FlashSR, returning 48kHz samples.
fn enhance_channel(
    session: &mut ort::session::Session,
    samples: &[f32],
    original_rate: u32,
    cancelled: CancelCheck<'_>,
) -> Result<Vec<f32>, String> {
    check_cancelled(cancelled)?;
    let samples_16k = resample_linear(samples, original_rate, 16000);

    if samples_16k.is_empty() {
        return Err("Empty audio for FlashSR".into());
    }

    let total_input = samples_16k.len();
    let total_output = total_input
        .checked_mul(FLASHSR_UPSAMPLE_FACTOR)
        .ok_or_else(|| "FlashSR output length overflowed".to_string())?;
    let overlap_output = FLASHSR_OVERLAP_INPUT_SAMPLES * FLASHSR_UPSAMPLE_FACTOR;
    let hop = FLASHSR_FRAME_SAMPLES - FLASHSR_OVERLAP_INPUT_SAMPLES;
    let mut combined = vec![0.0f32; total_output];
    let mut weights = vec![0.0f32; total_output];
    let mut position = 0usize;

    loop {
        check_cancelled(cancelled)?;
        let end = (position + FLASHSR_FRAME_SAMPLES).min(total_input);
        let frame = run_flashsr_frame(session, &samples_16k[position..end])?;
        check_cancelled(cancelled)?;

        let expected = (end - position) * FLASHSR_UPSAMPLE_FACTOR;
        if frame.len() < expected {
            return Err(format!(
                "FlashSR returned {} samples for a frame that requires {expected}",
                frame.len()
            ));
        }
        let is_first = position == 0;
        let is_last = end == total_input;
        let output_position = position * FLASHSR_UPSAMPLE_FACTOR;
        for (index, sample) in frame[..expected].iter().enumerate() {
            let weight = overlap_weight(index, expected, overlap_output, is_first, is_last);
            combined[output_position + index] += sample * weight;
            weights[output_position + index] += weight;
        }

        if is_last {
            break;
        }
        position += hop;
    }

    for (sample, weight) in combined.iter_mut().zip(weights) {
        if weight > 0.0 {
            *sample /= weight;
        }
    }
    Ok(combined)
}

fn run_flashsr_frame(session: &mut ort::session::Session, samples: &[f32]) -> Result<Vec<f32>, String> {
    let mut input = samples.to_vec();
    input.resize(input.len().max(FLASHSR_MIN_FRAME_SAMPLES), 0.0);
    let input_len = input.len();
    let input_tensor =
        ndarray::Array2::from_shape_vec((1, input_len), input).map_err(|e| format!("Tensor error: {}", e))?;
    let input_val = ort::value::Tensor::from_array(input_tensor).map_err(|e| format!("Tensor error: {}", e))?;

    let outputs = session
        .run(ort::inputs!["audio_values" => input_val])
        .map_err(|e| format!("FlashSR inference failed: {}", e))?;

    let first_output = outputs.values().next().ok_or("No FlashSR output")?;
    let output_tensor = first_output
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("Failed to extract FlashSR output: {}", e))?;

    if output_tensor.1.is_empty() {
        return Err("FlashSR returned an empty output".into());
    }
    Ok(output_tensor.1.to_vec())
}

fn overlap_weight(index: usize, len: usize, overlap: usize, is_first: bool, is_last: bool) -> f32 {
    if overlap <= 1 || len <= 1 {
        return 1.0;
    }
    let overlap = overlap.min(len);
    let denominator = (overlap - 1).max(1) as f32;
    let fade_in = if !is_first && index < overlap {
        index as f32 / denominator
    } else {
        1.0
    };
    let fade_out = if !is_last && index >= len - overlap {
        (len - 1 - index) as f32 / denominator
    } else {
        1.0
    };
    fade_in.min(fade_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjacent_flashsr_frame_windows_sum_to_unity() {
        let overlap = FLASHSR_OVERLAP_INPUT_SAMPLES * FLASHSR_UPSAMPLE_FACTOR;
        let frame_len = FLASHSR_FRAME_SAMPLES * FLASHSR_UPSAMPLE_FACTOR;
        for index in 0..overlap {
            let previous = overlap_weight(frame_len - overlap + index, frame_len, overlap, true, false);
            let next = overlap_weight(index, frame_len, overlap, false, true);
            assert!((previous + next - 1.0).abs() < 1e-6);
        }
    }

    #[test]
    fn outer_flashsr_edges_keep_full_weight() {
        let overlap = FLASHSR_OVERLAP_INPUT_SAMPLES * FLASHSR_UPSAMPLE_FACTOR;
        let frame_len = FLASHSR_FRAME_SAMPLES * FLASHSR_UPSAMPLE_FACTOR;
        assert_eq!(overlap_weight(0, frame_len, overlap, true, false), 1.0);
        assert_eq!(overlap_weight(frame_len - 1, frame_len, overlap, false, true), 1.0);
    }
}
