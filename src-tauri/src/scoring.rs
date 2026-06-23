use std::path::Path;

use tauri::AppHandle;

use crate::models;

// ── DNSMOS quality scoring ──────────────────────────────────────────────────
//
// Uses Microsoft's DNSMOS (Deep Noise Suppression Mean Opinion Score) model
// to rate speech quality on a 1–5 scale. Shows users how much their audio
// improved after processing.
//
// Outputs three scores:
//   - SIG: speech signal quality (1-5)
//   - BAK: background noise quality (1-5, higher = less noise)
//   - OVR: overall quality (1-5)

/// Quality scores from DNSMOS analysis.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QualityScore {
    /// Speech signal quality (1-5)
    pub sig: f32,
    /// Background noise quality (1-5, higher = cleaner)
    pub bak: f32,
    /// Overall quality (1-5)
    pub ovr: f32,
}

/// Score audio quality using the DNSMOS model.
/// Input should be 16kHz mono WAV audio.
pub(crate) async fn score_quality(
    app: &AppHandle,
    audio_path: &Path,
) -> Result<QualityScore, String> {
    let model_path = models::model_path(app, "dnsmos_sig_bak_ovr.onnx")?;
    let mut session = models::load_session(&model_path)?;

    // Decode to 16kHz mono for DNSMOS (drop guard cleans up on every exit path)
    let tmp = crate::safety::TempFile::new(std::env::temp_dir().join(format!(
        "depoaudio_score_{}.wav",
        uuid::Uuid::new_v4().to_string().replace('-', "")
    )));

    // DNSMOS only scores the first ~9s, so decode a short head — no need to
    // read (and no risk of hanging on) the whole recording.
    let args: Vec<String> = vec![
        "-t".into(), "12".into(),
        "-i".into(), audio_path.to_string_lossy().to_string(),
        "-af".into(), "aresample=16000".into(),
        "-ac".into(), "1".into(),
        "-acodec".into(), "pcm_s16le".into(),
        "-y".into(), tmp.to_string_lossy().to_string(),
    ];

    let output = crate::ffmpeg::sidecar_output_opt(app, crate::helpers::ffmpeg_bin_name(), args, 60)
        .await
        .ok_or_else(|| "Failed to decode audio for quality scoring".to_string())?;

    if !output.status.success() {
        return Err("Failed to decode audio for quality scoring".into());
    }

    // Read the 16kHz mono WAV
    let reader = hound::WavReader::open(&tmp)
        .map_err(|e| format!("Failed to open WAV: {}", e))?;

    let samples: Vec<f32> = reader
        .into_samples::<i16>()
        .filter_map(|s| s.ok())
        .map(|s| s as f32 / 32768.0)
        .collect();

    drop(tmp);

    if samples.is_empty() {
        return Err("No audio samples for quality scoring".into());
    }

    // DNSMOS expects a fixed-length input. Use first 9.01 seconds (144160 samples at 16kHz)
    // or pad with zeros if shorter.
    let target_len = 144160;
    let mut input_samples = samples;
    if input_samples.len() > target_len {
        input_samples.truncate(target_len);
    } else {
        input_samples.resize(target_len, 0.0);
    }

    // Create input tensor [1, target_len]
    let input = ndarray::Array2::from_shape_vec((1, target_len), input_samples)
        .map_err(|e| format!("Tensor error: {}", e))?;
    let input_val = ort::value::Tensor::from_array(input)
        .map_err(|e| format!("Tensor error: {}", e))?;

    let outputs = session
        .run(ort::inputs!["input_1" => input_val])
        .map_err(|e| format!("DNSMOS inference failed: {}", e))?;

    // Output is [1, 3] with [SIG, BAK, OVR] scores
    let first_output = outputs
        .values()
        .next()
        .ok_or("No DNSMOS output")?;
    let scores = first_output
        .try_extract_tensor::<f32>()
        .map_err(|e| format!("Failed to extract DNSMOS output: {}", e))?;

    let scores_slice = scores.1;

    if scores_slice.len() >= 3 {
        Ok(QualityScore {
            sig: scores_slice[0].clamp(1.0, 5.0),
            bak: scores_slice[1].clamp(1.0, 5.0),
            ovr: scores_slice[2].clamp(1.0, 5.0),
        })
    } else if scores_slice.len() == 1 {
        // Some DNSMOS variants output a single OVR score
        let ovr = scores_slice[0].clamp(1.0, 5.0);
        Ok(QualityScore { sig: ovr, bak: ovr, ovr })
    } else {
        Err("Unexpected DNSMOS output shape".into())
    }
}
