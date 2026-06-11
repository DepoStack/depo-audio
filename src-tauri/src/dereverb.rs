use tauri::AppHandle;

use crate::helpers::resample_linear;
use crate::models;
use crate::types::AudioBuffer;

// ── De-reverberation (DCCRN+) ───────────────────────────────────────────────
//
// Reduces room echo and reverb from speech recordings using the DCCRN+
// (Deep Complex Convolution Recurrent Network) model.
//
// The model operates in the STFT domain on complex spectrograms, handling
// both noise and mild reverberation in a single pass.
//
// Model file: dccrn_plus.onnx (~20MB, must be exported from PyTorch)
// Input: 16kHz mono audio
// Output: Enhanced 16kHz mono audio
//
// See scripts/export_dccrn.py for model export instructions.

/// Apply de-reverberation to an AudioBuffer in-place.
/// The DCCRN+ model expects 16kHz mono input, so each channel is resampled,
/// processed independently, and resampled back. Channels are never downmixed:
/// in multi-channel court recordings each channel is a separate speaker mic,
/// and mixing them would destroy per-speaker separation.
pub(crate) fn dereverb_buffer(
    app: &AppHandle,
    buf: &mut AudioBuffer,
) -> Result<(), String> {
    let model_path = models::model_path(app, "dccrn_plus.onnx")?;
    let mut session = models::load_session(&model_path)?;

    let original_rate = buf.sample_rate;
    let channel_bufs = buf.channels_split();
    let mut processed = Vec::with_capacity(channel_bufs.len());
    for ch_samples in &channel_bufs {
        processed.push(dereverb_channel(&mut session, ch_samples, original_rate)?);
    }
    *buf = AudioBuffer::from_channels(&processed, original_rate);

    Ok(())
}

/// Run one mono channel through DCCRN+, returning samples at the input rate.
fn dereverb_channel(
    session: &mut ort::session::Session,
    samples: &[f32],
    original_rate: u32,
) -> Result<Vec<f32>, String> {
    let samples_16k = resample_linear(samples, original_rate, 16000);

    if samples_16k.is_empty() {
        return Err("Empty audio for de-reverb".into());
    }

    // DCCRN+ processes in fixed-length frames
    let frame_length = 32000usize; // 2 seconds at 16kHz
    let hop = 16000usize; // 1 second overlap
    let total_samples = samples_16k.len();

    let mut output_samples = vec![0.0f32; total_samples];
    let mut weight_sum = vec![0.0f32; total_samples];

    let mut pos = 0usize;
    while pos < total_samples {
        let end = (pos + frame_length).min(total_samples);
        let mut frame = samples_16k[pos..end].to_vec();

        if frame.len() < frame_length {
            frame.resize(frame_length, 0.0);
        }

        let input_tensor = ndarray::Array2::from_shape_vec((1, frame_length), frame)
            .map_err(|e| format!("Tensor error: {}", e))?;
        let input_val = ort::value::Tensor::from_array(input_tensor)
            .map_err(|e| format!("Tensor error: {}", e))?;

        let result = session.run(ort::inputs!["input" => input_val]);

        match result {
            Ok(outputs) => {
                if let Some(out_val) = outputs.values().next() {
                    if let Ok(out_tensor) = out_val.try_extract_tensor::<f32>() {
                        let out_data = out_tensor.1;
                        let actual_len = (end - pos).min(out_data.len());

                        for j in 0..actual_len {
                            let w = if j < hop {
                                j as f32 / hop as f32
                            } else {
                                1.0 - ((j - hop) as f32 / hop as f32).min(1.0)
                            };
                            output_samples[pos + j] += out_data[j] * w;
                            weight_sum[pos + j] += w;
                        }
                    }
                }
            }
            Err(_) => {
                let actual_len = end - pos;
                for j in 0..actual_len {
                    output_samples[pos + j] += samples_16k[pos + j];
                    weight_sum[pos + j] += 1.0;
                }
            }
        }

        pos += hop;
    }

    // Normalize by weight sum
    for i in 0..total_samples {
        if weight_sum[i] > 0.0 {
            output_samples[i] /= weight_sum[i];
        }
    }

    // Resample back to original rate if needed
    Ok(resample_linear(&output_samples, 16000, original_rate))
}
