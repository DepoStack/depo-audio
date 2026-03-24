use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;
use uuid::Uuid;

use crate::models;

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

/// Check if de-reverb is available (model file exists).
pub(crate) fn is_available(app: &AppHandle) -> bool {
    models::model_path(app, "dccrn_plus.onnx").is_ok()
}

/// Apply de-reverberation to an audio file.
/// Returns the path to the processed temp WAV file.
pub(crate) async fn dereverb(
    app: &AppHandle,
    input: &Path,
) -> Result<PathBuf, String> {
    let model_path = models::model_path(app, "dccrn_plus.onnx")?;
    let mut session = models::load_session(&model_path)?;

    // Decode to 16kHz mono WAV
    let tmp_in = std::env::temp_dir().join(format!(
        "depoaudio_dereverb_in_{}.wav",
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
        return Err("Failed to decode audio for de-reverb".into());
    }

    let reader = hound::WavReader::open(&tmp_in)
        .map_err(|e| format!("WAV read error: {}", e))?;
    let spec = reader.spec();
    let samples: Vec<f32> = reader
        .into_samples::<f32>()
        .filter_map(|s| s.ok())
        .collect();

    let _ = std::fs::remove_file(&tmp_in);

    if samples.is_empty() {
        return Err("Empty audio for de-reverb".into());
    }

    // DCCRN+ processes audio in fixed-length frames
    // Input: [1, frame_length] of time-domain samples
    // Output: [1, frame_length] of enhanced time-domain samples
    let frame_length = 32000; // 2 seconds at 16kHz
    let hop = 16000; // 1 second overlap
    let total_samples = samples.len();

    let mut output_samples = vec![0.0f32; total_samples];
    let mut weight_sum = vec![0.0f32; total_samples];

    let mut pos = 0usize;
    while pos < total_samples {
        let end = (pos + frame_length).min(total_samples);
        let mut frame = samples[pos..end].to_vec();

        // Pad short frames
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

                        // Overlap-add with triangular window
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
                // Pass through original on inference failure
                let actual_len = end - pos;
                for j in 0..actual_len {
                    output_samples[pos + j] += samples[pos + j];
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

    // Write output WAV
    let tmp_out = std::env::temp_dir().join(format!(
        "depoaudio_dereverb_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let out_spec = hound::WavSpec {
        channels: 1,
        sample_rate: spec.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(&tmp_out, out_spec)
        .map_err(|e| format!("WAV write error: {}", e))?;
    for &s in &output_samples {
        writer.write_sample(s).map_err(|e| format!("Write error: {}", e))?;
    }
    writer.finalize().map_err(|e| format!("Finalize error: {}", e))?;

    Ok(tmp_out)
}
