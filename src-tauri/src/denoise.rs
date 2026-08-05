use std::path::{Path, PathBuf};

use nnnoiseless::DenoiseState;
use uuid::Uuid;

use crate::ffmpeg::{sidecar_output_cancellable, CancelCheck};
use crate::types::{AudioBuffer, CONVERSION_CANCELLED_MESSAGE};

const DENOISE_DECODE_TIMEOUT_SECS: u64 = 20 * 60;

// ── Audio denoising via nnnoiseless (RNNoise) ───────────────────────────────
//
// Processes audio through a neural noise gate that suppresses background noise
// (HVAC, paper rustling, room tone) while keeping speech clear.
// Works per-channel for multi-channel files.

/// Frame size expected by RNNoise (480 samples at 48 kHz = 10 ms).
const FRAME_SIZE: usize = DenoiseState::FRAME_SIZE;

/// Denoise an AudioBuffer in-place. Expects 48kHz input.
pub(crate) fn denoise_buffer(buf: &mut AudioBuffer, cancelled: CancelCheck<'_>) -> Result<(), String> {
    if buf.sample_rate != 48000 {
        return Err(format!("Denoise requires 48kHz, got {}Hz", buf.sample_rate));
    }
    let mut channel_bufs = buf.channels_split();
    for ch_buf in &mut channel_bufs {
        denoise_channel(ch_buf, cancelled)?;
    }
    *buf = AudioBuffer::from_channels(&channel_bufs, buf.sample_rate);
    Ok(())
}

/// Process a single channel through RNNoise.
/// Operates in-place on the sample buffer.
fn denoise_channel(samples: &mut Vec<f32>, cancelled: CancelCheck<'_>) -> Result<(), String> {
    let mut state = DenoiseState::new();
    let mut frame = [0.0f32; FRAME_SIZE];

    // Pad to a multiple of FRAME_SIZE
    let original_len = samples.len();
    let remainder = original_len % FRAME_SIZE;
    if remainder != 0 {
        samples.extend(std::iter::repeat_n(0.0f32, FRAME_SIZE - remainder));
    }

    let num_frames = samples.len() / FRAME_SIZE;

    for i in 0..num_frames {
        if cancelled.map(|check| check()).unwrap_or(false) {
            samples.truncate(original_len);
            return Err(CONVERSION_CANCELLED_MESSAGE.into());
        }
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

        // Write back, converting from i16 scale to f32. RNNoise can emit peaks
        // slightly above the input scale, so clamp to [-1, 1] — keep/split
        // modes have no limiter after denoise and would otherwise hard-clip.
        for j in 0..FRAME_SIZE {
            samples[offset + j] = (frame[j] / 32767.0).clamp(-1.0, 1.0);
        }
    }

    // Trim back to original length
    samples.truncate(original_len);
    Ok(())
}

/// Decode any audio file to a 48 kHz WAV using FFmpeg, suitable for denoising.
/// `input_codec` carries input-decoder options (notably `-c:a ftr` for FTR's
/// modified multichannel bitstream); pass an empty slice otherwise.
/// Returns the path to the decoded temp WAV file.
pub(crate) async fn decode_to_wav_48k(
    app: &tauri::AppHandle,
    input: &Path,
    input_codec: &[String],
    cancelled: CancelCheck<'_>,
) -> Result<PathBuf, String> {
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_dec_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file".into(),
    ];
    args.extend(input_codec.iter().cloned());
    args.extend([
        "-i".into(),
        input.to_string_lossy().to_string(),
        "-ar".into(),
        "48000".into(),
        "-acodec".into(),
        "pcm_f32le".into(),
        "-y".into(),
        tmp.to_string_lossy().to_string(),
    ]);

    let output = sidecar_output_cancellable(
        app,
        crate::helpers::ffmpeg_bin_name(),
        args,
        DENOISE_DECODE_TIMEOUT_SECS,
        cancelled,
    )
    .await
    .ok_or_else(|| {
        let _ = std::fs::remove_file(&tmp);
        if cancelled.map(|check| check()).unwrap_or(false) {
            CONVERSION_CANCELLED_MESSAGE.to_string()
        } else {
            "FFmpeg decode timed out or could not start".to_string()
        }
    })?;

    if !output.success {
        let _ = std::fs::remove_file(&tmp);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.replace(['/', '\\'], "_").chars().take(200).collect::<String>();
        return Err(format!("FFmpeg decode failed: {detail}"));
    }

    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rnnoise_honors_cancellation_between_frames() {
        let mut buffer = AudioBuffer {
            samples: vec![0.0; FRAME_SIZE * 2],
            channels: 1,
            sample_rate: 48_000,
        };
        let cancelled = || true;

        let error = denoise_buffer(&mut buffer, Some(&cancelled)).unwrap_err();

        assert_eq!(error, CONVERSION_CANCELLED_MESSAGE);
        assert_eq!(buffer.samples.len(), FRAME_SIZE * 2);
    }
}
