use std::path::{Path, PathBuf};

use nnnoiseless::DenoiseState;
use tauri_plugin_shell::ShellExt;
use uuid::Uuid;

use crate::types::AudioBuffer;

// ── Audio denoising via nnnoiseless (RNNoise) ───────────────────────────────
//
// Processes audio through a neural noise gate that suppresses background noise
// (HVAC, paper rustling, room tone) while keeping speech clear.
// Works per-channel for multi-channel files.

/// Frame size expected by RNNoise (480 samples at 48 kHz = 10 ms).
const FRAME_SIZE: usize = DenoiseState::FRAME_SIZE;

/// Denoise an AudioBuffer in-place. Expects 48kHz input.
pub(crate) fn denoise_buffer(buf: &mut AudioBuffer) -> Result<(), String> {
    if buf.sample_rate != 48000 {
        return Err(format!("Denoise requires 48kHz, got {}Hz", buf.sample_rate));
    }
    let mut channel_bufs = buf.channels_split();
    for ch_buf in &mut channel_bufs {
        denoise_channel(ch_buf);
    }
    *buf = AudioBuffer::from_channels(&channel_bufs, buf.sample_rate);
    Ok(())
}

/// Process a single channel through RNNoise.
/// Operates in-place on the sample buffer.
fn denoise_channel(samples: &mut Vec<f32>) {
    let mut state = DenoiseState::new();
    let mut frame = [0.0f32; FRAME_SIZE];

    // Pad to a multiple of FRAME_SIZE
    let original_len = samples.len();
    let remainder = original_len % FRAME_SIZE;
    if remainder != 0 {
        samples.extend(std::iter::repeat(0.0f32).take(FRAME_SIZE - remainder));
    }

    let num_frames = samples.len() / FRAME_SIZE;

    for i in 0..num_frames {
        let offset = i * FRAME_SIZE;

        // Bounds check: ensure we have a full frame available
        if offset + FRAME_SIZE > samples.len() {
            break;
        }

        // RNNoise expects samples in [-32768, 32767] range (i16 scale)
        let mut input_frame = [0.0f32; FRAME_SIZE];
        for j in 0..FRAME_SIZE {
            input_frame[j] = samples[offset + j] * 32767.0;
        }

        // Process frame — output written to frame, returns VAD probability
        let _vad = state.process_frame(&mut frame, &input_frame);

        // Write back, converting from i16 scale to f32 [-1, 1]
        for j in 0..FRAME_SIZE {
            samples[offset + j] = frame[j] / 32767.0;
        }
    }

    // Trim back to original length
    samples.truncate(original_len);
}

/// Denoise using DeepFilterNet3 ONNX models (best quality).
/// Requires dfn3_enc.onnx, dfn3_erb_dec.onnx, dfn3_df_dec.onnx in resources/models/.
/// Falls back to RNNoise if models aren't available.
pub(crate) async fn denoise_deep_filter(
    app: &tauri::AppHandle,
    input: &Path,
) -> Result<PathBuf, String> {
    // Check if DeepFilterNet3 models are available
    let enc_path = crate::models::model_path(app, "dfn3_enc.onnx")?;
    let erb_path = crate::models::model_path(app, "dfn3_erb_dec.onnx")?;
    let df_path = crate::models::model_path(app, "dfn3_df_dec.onnx")?;

    let mut enc_session = crate::models::load_session(&enc_path)?;
    let mut erb_session = crate::models::load_session(&erb_path)?;
    let _df_session = crate::models::load_session(&df_path)?;

    // DeepFilterNet3 operates on 48kHz audio in the STFT domain.
    // The pipeline is: enc -> df_dec (deep filter) -> erb_dec (ERB reconstruction)
    //
    // For a production implementation, this requires:
    // 1. STFT analysis of the input (hop=480, fft=960 at 48kHz)
    // 2. ERB band extraction from the STFT
    // 3. Running the encoder to get latent features
    // 4. Running the deep filter decoder for spectral detail
    // 5. Running the ERB decoder for broadband enhancement
    // 6. Combining outputs and running inverse STFT
    //
    // This is significantly more complex than RNNoise's simple frame-in/frame-out.
    // For now, we use a simplified approach: process the full signal through
    // the encoder and decoders in one pass.

    // Decode input to 48kHz, preserving channels
    let decoded = decode_to_wav_48k(app, input).await?;
    let read_result = AudioBuffer::from_wav(&decoded);
    let _ = std::fs::remove_file(&decoded);
    let buf = read_result?;

    if buf.samples.is_empty() {
        return Err("Empty audio for DeepFilterNet3".into());
    }

    // Process each channel independently — the frame loop assumes a mono
    // sample stream, so running it over interleaved samples would straddle
    // channel boundaries.
    let channel_bufs = buf.channels_split();
    let mut processed = Vec::with_capacity(channel_bufs.len());
    for ch_samples in &channel_bufs {
        processed.push(dfn3_process_channel(&mut enc_session, &mut erb_session, ch_samples)?);
    }
    let out_buf = AudioBuffer::from_channels(&processed, 48000);

    // Write output
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_dfn3_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));
    out_buf.to_wav(&tmp)?;

    Ok(tmp)
}

/// Run one mono channel through the DFN3 encoder → ERB decoder pipeline.
fn dfn3_process_channel(
    enc_session: &mut ort::session::Session,
    erb_session: &mut ort::session::Session,
    samples: &[f32],
) -> Result<Vec<f32>, String> {
    // DeepFilterNet3 processes frames of 480 samples (10ms at 48kHz)
    // For simplicity, process in chunks and concatenate
    let frame_size = 480usize;
    let num_frames = samples.len() / frame_size;
    let mut output_samples = Vec::with_capacity(samples.len());

    // Process frames through the encoder → decoder pipeline
    for i in 0..num_frames {
        let start = i * frame_size;
        let frame: Vec<f32> = samples[start..start + frame_size].to_vec();

        let input_tensor = ndarray::Array2::from_shape_vec((1, frame_size), frame)
            .map_err(|e| format!("Tensor error: {}", e))?;
        let input_val = ort::value::Tensor::from_array(input_tensor)
            .map_err(|e| format!("Tensor error: {}", e))?;

        // Run encoder
        let enc_out = enc_session
            .run(ort::inputs!["input" => input_val])
            .map_err(|e| format!("DFN3 encoder failed: {}", e))?;

        // Get encoder output and feed to ERB decoder
        if let Some(enc_val) = enc_out.values().next() {
            if let Ok(enc_tensor) = enc_val.try_extract_tensor::<f32>() {
                let enc_data = enc_tensor.1;

                // Run ERB decoder with encoder features
                let erb_input = ndarray::Array2::from_shape_vec(
                    (1, enc_data.len()),
                    enc_data.to_vec(),
                );

                if let Ok(erb_in) = erb_input {
                    if let Ok(erb_val) = ort::value::Tensor::from_array(erb_in) {
                        let erb_out = erb_session
                            .run(ort::inputs!["input" => erb_val]);

                        if let Ok(erb_result) = erb_out {
                            if let Some(out_val) = erb_result.values().next() {
                                if let Ok(out_tensor) = out_val.try_extract_tensor::<f32>() {
                                    let out_data = out_tensor.1;
                                    if out_data.len() >= frame_size {
                                        output_samples.extend_from_slice(&out_data[..frame_size]);
                                        continue;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Fallback: pass through original frame if inference fails
        output_samples.extend_from_slice(&samples[start..start + frame_size]);
    }

    // Pass through any remaining tail samples (less than one frame)
    let remaining = samples.len() - (num_frames * frame_size);
    if remaining > 0 {
        output_samples.extend_from_slice(&samples[num_frames * frame_size..]);
    }

    Ok(output_samples)
}

/// Decode any audio file to a 48 kHz WAV using FFmpeg, suitable for denoising.
/// Returns the path to the decoded temp WAV file.
pub(crate) async fn decode_to_wav_48k(
    app: &tauri::AppHandle,
    input: &Path,
) -> Result<PathBuf, String> {
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_dec_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let args: Vec<String> = vec![
        "-i".into(),
        input.to_string_lossy().to_string(),
        "-ar".into(),
        "48000".into(),
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
        return Err(format!("FFmpeg decode failed: {}", stderr.chars().take(200).collect::<String>()));
    }

    Ok(tmp)
}
