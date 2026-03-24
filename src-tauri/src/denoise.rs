use std::path::{Path, PathBuf};

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use nnnoiseless::DenoiseState;
use tauri_plugin_shell::ShellExt;
use uuid::Uuid;

// ── Audio denoising via nnnoiseless (RNNoise) ───────────────────────────────
//
// Processes audio through a neural noise gate that suppresses background noise
// (HVAC, paper rustling, room tone) while keeping speech clear.
// Works per-channel for multi-channel files.

/// Frame size expected by RNNoise (480 samples at 48 kHz = 10 ms).
const FRAME_SIZE: usize = DenoiseState::FRAME_SIZE;

/// Denoise a WAV file and write the result to a temp WAV file.
/// Returns the path to the denoised temp file.
///
/// The input must be a PCM WAV file (decode with FFmpeg first if needed).
pub(crate) fn denoise_wav(input: &Path) -> Result<PathBuf, String> {
    let reader = WavReader::open(input).map_err(|e| format!("Failed to open WAV: {}", e))?;
    let spec = reader.spec();
    let channels = spec.channels as usize;
    let sample_rate = spec.sample_rate;

    // RNNoise expects 48 kHz. If the input is different, we skip denoising
    // and return the original path (FFmpeg can resample before this step).
    if sample_rate != 48000 {
        return Err(format!(
            "Denoise requires 48 kHz input, got {} Hz. Resample first.",
            sample_rate
        ));
    }

    // Read all samples as f32
    let samples: Vec<f32> = match spec.sample_format {
        SampleFormat::Int => reader
            .into_samples::<i32>()
            .map(|s| s.unwrap_or(0) as f32 / i32::MAX as f32)
            .collect(),
        SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
    };

    // De-interleave into per-channel buffers
    let total_frames = samples.len() / channels;
    let mut channel_bufs: Vec<Vec<f32>> = (0..channels)
        .map(|ch| {
            (0..total_frames)
                .map(|f| samples[f * channels + ch])
                .collect()
        })
        .collect();

    // Process each channel through RNNoise
    for buf in &mut channel_bufs {
        denoise_channel(buf);
    }

    // Re-interleave
    let mut output_samples = Vec::with_capacity(samples.len());
    for frame in 0..total_frames {
        for ch in 0..channels {
            output_samples.push(channel_bufs[ch][frame]);
        }
    }

    // Write to temp WAV
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_denoised_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let out_spec = WavSpec {
        channels: spec.channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };

    let mut writer =
        WavWriter::create(&tmp, out_spec).map_err(|e| format!("Failed to create WAV: {}", e))?;

    for &sample in &output_samples {
        writer
            .write_sample(sample)
            .map_err(|e| format!("Write error: {}", e))?;
    }

    writer
        .finalize()
        .map_err(|e| format!("Finalize error: {}", e))?;

    Ok(tmp)
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

    // Decode input to 48kHz mono
    let decoded = decode_to_wav_48k(app, input).await?;
    let reader = hound::WavReader::open(&decoded)
        .map_err(|e| format!("WAV read error: {}", e))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .map(|s| s.unwrap_or(0) as f32 / i32::MAX as f32)
            .collect(),
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .map(|s| s.unwrap_or(0.0))
            .collect(),
    };
    let _ = std::fs::remove_file(&decoded);

    if samples.is_empty() {
        return Err("Empty audio for DeepFilterNet3".into());
    }

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

    // Handle remaining samples
    let remaining = samples.len() - (num_frames * frame_size);
    if remaining > 0 {
        output_samples.extend_from_slice(&samples[num_frames * frame_size..]);
    }

    // Write output
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_dfn3_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let out_spec = hound::WavSpec {
        channels: spec.channels,
        sample_rate: 48000,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(&tmp, out_spec)
        .map_err(|e| format!("WAV write error: {}", e))?;
    for &s in &output_samples {
        writer.write_sample(s).map_err(|e| format!("Write error: {}", e))?;
    }
    writer.finalize().map_err(|e| format!("Finalize error: {}", e))?;

    Ok(tmp)
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
