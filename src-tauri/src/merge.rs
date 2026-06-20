use std::path::{Path, PathBuf};

use tauri::AppHandle;
use tauri_plugin_shell::ShellExt;
use uuid::Uuid;

use crate::helpers::ffmpeg_bin_name;

// ── Multi-source audio merge ────────────────────────────────────────────────
//
// Combines multiple recordings of the same event into one clean output.
// Typical use: court reporter mic + backup recorder + phone-in participant.
//
// Pipeline:
//   1. Decode all inputs to same sample rate (48kHz mono WAV)
//   2. Auto-detect timing offset via cross-correlation
//   3. Align tracks to a common timeline
//   4. Score quality per segment using RMS energy (speech clarity proxy)
//   5. Build output by selecting the cleanest source per segment
//   6. Crossfade at transition points for smooth blending

/// Configuration for a merge job.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MergeJob {
    /// Paths to audio files to merge.
    pub sources: Vec<String>,
    /// Output directory.
    pub out_dir: String,
    /// Output filename (without extension).
    pub out_name: String,
    /// Output format: wav, mp3, flac, opus.
    pub format: String,
    /// Sample rate for output.
    pub rate: String,
    /// Merge strategy: "best_quality" or "mix_all".
    pub strategy: String,
}

/// Result of analyzing sync between two audio files.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SyncResult {
    /// Detected offset in seconds (positive = source B starts later).
    pub offset_seconds: f64,
    /// Confidence of the sync detection (0.0 - 1.0).
    pub confidence: f64,
    /// Whether the recordings appear to be from the same event.
    pub is_same_event: bool,
}

/// Result of a merge operation.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub output_path: String,
    pub output_name: String,
    pub output_size: u64,
    pub duration: f64,
    pub sources_used: usize,
    pub sync_offsets: Vec<f64>,
}

// ── Sync detection ──────────────────────────────────────────────────────────

/// Detect the timing offset between two audio files using cross-correlation.
/// Returns the offset in seconds that aligns source_b to source_a.
pub(crate) async fn detect_sync(
    app: &AppHandle,
    source_a: &str,
    source_b: &str,
) -> Result<SyncResult, String> {
    // Decode both to 16kHz mono (lower rate = faster correlation)
    let tmp_a = decode_to_mono(app, source_a, 16000).await?;
    let tmp_b = decode_to_mono(app, source_b, 16000).await?;

    let samples_a = read_wav_samples(&tmp_a)?;
    let samples_b = read_wav_samples(&tmp_b)?;

    let _ = std::fs::remove_file(&tmp_a);
    let _ = std::fs::remove_file(&tmp_b);

    if samples_a.is_empty() || samples_b.is_empty() {
        return Err("One or both files are empty".into());
    }

    // Cross-correlate using a segment from source_a (first 30 seconds)
    let sample_rate = 16000usize;
    let search_len = (30 * sample_rate).min(samples_a.len()).min(samples_b.len());
    let max_offset = 60 * sample_rate; // Search up to 60 seconds offset

    let segment_a = &samples_a[..search_len];

    let mut best_corr = 0.0f64;
    let mut best_offset = 0i64;

    // Coarse search: stride of 1600 samples (100ms at 16kHz)
    let coarse_stride = 1600;
    let search_range = max_offset.min(samples_b.len());

    let neg_range = -(search_range as i64);
    let pos_range = search_range as i64;
    let mut coarse_offset = neg_range;
    while coarse_offset < pos_range {
        let offset = coarse_offset;
        coarse_offset += coarse_stride as i64;
        let corr = cross_correlate(segment_a, &samples_b, offset, search_len);
        if corr > best_corr {
            best_corr = corr;
            best_offset = offset;
        }
    }

    // Fine search: ±coarse_stride around best coarse offset
    let fine_start = best_offset - coarse_stride as i64;
    let fine_end = best_offset + coarse_stride as i64;
    for offset in fine_start..=fine_end {
        let corr = cross_correlate(segment_a, &samples_b, offset, search_len);
        if corr > best_corr {
            best_corr = corr;
            best_offset = offset;
        }
    }

    // cross_correlate matches a[i] against b[i + offset], so a positive
    // best_offset means B's content occurs *earlier* than A's. Negate to get
    // the documented semantics: positive = source B starts later.
    let offset_seconds = -(best_offset as f64) / sample_rate as f64;

    // Confidence from normalized cross-correlation: corr / (|a| * |b|), with
    // |b| measured over the overlapping window at the best offset.
    // energy_a must be measured over the SAME overlapping window as the
    // correlation (and energy_b). Using the full segment inflates the
    // denominator when the offset is large (small overlap), pushing genuine
    // matches below the is_same_event threshold.
    let energy_a: f64 = (0..search_len)
        .filter_map(|i| {
            let idx = i as i64 + best_offset;
            if idx >= 0 && (idx as usize) < samples_b.len() {
                let v = segment_a[i] as f64;
                Some(v * v)
            } else {
                None
            }
        })
        .sum();
    let energy_b: f64 = (0..search_len)
        .filter_map(|i| {
            let idx = i as i64 + best_offset;
            if idx >= 0 && (idx as usize) < samples_b.len() {
                let v = samples_b[idx as usize] as f64;
                Some(v * v)
            } else {
                None
            }
        })
        .sum();
    let confidence = if energy_a > 0.0 && energy_b > 0.0 {
        (best_corr / (energy_a.sqrt() * energy_b.sqrt())).min(1.0).max(0.0)
    } else {
        0.0
    };

    // Same-event threshold. Confidence is now a true normalized cross-
    // correlation of raw waveforms from different microphones, which sits
    // well below the old saturated values even for genuine matches — 0.15
    // separates matched recordings (~0.2+) from unrelated audio (~0.05).
    let is_same_event = confidence > 0.15;

    Ok(SyncResult {
        offset_seconds,
        confidence,
        is_same_event,
    })
}

/// Cross-correlate segment_a with samples_b at the given offset.
fn cross_correlate(segment_a: &[f32], samples_b: &[f32], offset: i64, len: usize) -> f64 {
    let mut sum = 0.0f64;
    for i in 0..len {
        let b_idx = i as i64 + offset;
        if b_idx >= 0 && (b_idx as usize) < samples_b.len() {
            sum += segment_a[i] as f64 * samples_b[b_idx as usize] as f64;
        }
    }
    sum.abs()
}

// ── Merge execution ─────────────────────────────────────────────────────────

/// Merge multiple audio files into one synchronized output.
pub(crate) async fn merge_audio(
    app: &AppHandle,
    job: &MergeJob,
) -> Result<MergeResult, String> {
    if job.sources.len() < 2 {
        return Err("Need at least 2 files to merge".into());
    }

    // Step 1: Detect sync offsets relative to the first source
    let mut offsets = vec![0.0f64]; // First source is reference (offset = 0)
    for i in 1..job.sources.len() {
        let sync = detect_sync(app, &job.sources[0], &job.sources[i]).await?;
        offsets.push(sync.offset_seconds);
    }

    // Step 2: Decode all sources to 48kHz mono
    let mut decoded_paths = Vec::new();
    for src in &job.sources {
        let tmp = decode_to_mono(app, src, 48000).await?;
        decoded_paths.push(tmp);
    }

    // Step 3: Read all samples
    let mut all_samples: Vec<Vec<f32>> = Vec::new();
    for path in &decoded_paths {
        all_samples.push(read_wav_samples(path)?);
    }
    for path in &decoded_paths {
        let _ = std::fs::remove_file(path);
    }

    // Step 4: Align to common timeline
    let sample_rate = 48000usize;
    let min_offset = offsets.iter().cloned().fold(f64::INFINITY, f64::min);
    let adjusted_offsets: Vec<i64> = offsets.iter()
        .map(|&o| ((o - min_offset) * sample_rate as f64) as i64)
        .collect();

    // Find total duration (longest aligned track)
    let total_samples = all_samples.iter().enumerate()
        .map(|(i, s)| s.len() as i64 + adjusted_offsets[i])
        .max()
        .unwrap_or(0) as usize;

    // Step 5: Build merged output based on strategy
    let merged = match job.strategy.as_str() {
        "mix_all" => mix_all_strategy(&all_samples, &adjusted_offsets, total_samples),
        _ => best_quality_strategy(&all_samples, &adjusted_offsets, total_samples, sample_rate),
    };

    // Step 6: Write merged WAV then convert to target format via FFmpeg
    let tmp_wav = std::env::temp_dir().join(format!(
        "depoaudio_merged_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    write_wav(&tmp_wav, &merged, 48000)?;

    // Convert to target format
    let ext = crate::helpers::output_ext(&job.format);
    // Merge output uses the default MP3 bitrate; per-conversion bitrate selection
    // lives on the Convert tab.
    let out_codec = crate::helpers::output_args(&job.format, &job.rate, 192);

    let out_dir = if job.out_dir.is_empty() {
        Path::new(&job.sources[0]).parent().unwrap_or(Path::new(".")).to_path_buf()
    } else {
        PathBuf::from(&job.out_dir)
    };

    let out_name = if job.out_name.is_empty() { "merged".to_string() } else { job.out_name.clone() };
    let out_path = crate::helpers::unique_path(&out_dir.join(format!("{}{}", out_name, ext)));

    let mut args: Vec<String> = vec![
        "-i".into(), tmp_wav.to_string_lossy().to_string(),
    ];
    args.extend(out_codec);
    args.extend(["-y".into(), out_path.to_string_lossy().to_string()]);

    let output = app.shell()
        .sidecar(ffmpeg_bin_name())
        .map_err(|e| e.to_string())?
        .args(args)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    let _ = std::fs::remove_file(&tmp_wav);

    if output.status.code() != Some(0) {
        return Err("Failed to encode merged output".into());
    }

    let size = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
    let duration = merged.len() as f64 / 48000.0;

    Ok(MergeResult {
        output_path: out_path.to_string_lossy().to_string(),
        output_name: crate::helpers::basename(&out_path.to_string_lossy()),
        output_size: size,
        duration,
        sources_used: job.sources.len(),
        sync_offsets: offsets,
    })
}

// ── Merge strategies ────────────────────────────────────────────────────────

/// Mix all sources together with equal weight (simple average).
fn mix_all_strategy(
    sources: &[Vec<f32>],
    offsets: &[i64],
    total_samples: usize,
) -> Vec<f32> {
    let mut output = vec![0.0f32; total_samples];
    let mut counts = vec![0u32; total_samples];

    for (src_idx, samples) in sources.iter().enumerate() {
        let offset = offsets[src_idx];
        for (i, &s) in samples.iter().enumerate() {
            let out_idx = i as i64 + offset;
            if out_idx >= 0 && (out_idx as usize) < total_samples {
                output[out_idx as usize] += s;
                counts[out_idx as usize] += 1;
            }
        }
    }

    // Average where multiple sources overlap
    for i in 0..total_samples {
        if counts[i] > 1 {
            output[i] /= counts[i] as f32;
        }
    }

    output
}

/// Select the highest quality segment from available sources.
/// Uses RMS energy in speech-likely regions as a quality proxy.
fn best_quality_strategy(
    sources: &[Vec<f32>],
    offsets: &[i64],
    total_samples: usize,
    sample_rate: usize,
) -> Vec<f32> {
    let mut output = vec![0.0f32; total_samples];

    // Process in 500ms segments
    let segment_size = sample_rate / 2;
    let crossfade_len = sample_rate / 20; // 50ms crossfade

    let mut pos = 0usize;
    let mut prev_best: Option<usize> = None;

    while pos < total_samples {
        let end = (pos + segment_size).min(total_samples);

        // Find the best source for this segment (highest RMS)
        let mut best_src = 0;
        let mut best_rms = 0.0f64;

        for (src_idx, samples) in sources.iter().enumerate() {
            let offset = offsets[src_idx];
            let mut rms_sum = 0.0f64;
            let mut count = 0usize;

            for out_idx in pos..end {
                let src_idx_sample = out_idx as i64 - offset;
                if src_idx_sample >= 0 && (src_idx_sample as usize) < samples.len() {
                    let s = samples[src_idx_sample as usize] as f64;
                    rms_sum += s * s;
                    count += 1;
                }
            }

            let rms = if count > 0 { (rms_sum / count as f64).sqrt() } else { 0.0 };
            if rms > best_rms {
                best_rms = rms;
                best_src = src_idx;
            }
        }

        // Copy best source to output
        let offset = offsets[best_src];
        for out_idx in pos..end {
            let src_idx_sample = out_idx as i64 - offset;
            if src_idx_sample >= 0 && (src_idx_sample as usize) < sources[best_src].len() {
                output[out_idx] = sources[best_src][src_idx_sample as usize];
            }
        }

        // Apply crossfade if source changed
        if let Some(prev) = prev_best {
            if prev != best_src && pos > 0 {
                let fade_start = pos.saturating_sub(crossfade_len / 2);
                let fade_end = (pos + crossfade_len / 2).min(total_samples);
                let fade_len = fade_end - fade_start;

                for j in 0..fade_len {
                    let t = j as f32 / fade_len as f32;
                    let prev_offset = offsets[prev];
                    let prev_idx = (fade_start + j) as i64 - prev_offset;
                    let prev_sample = if prev_idx >= 0 && (prev_idx as usize) < sources[prev].len() {
                        sources[prev][prev_idx as usize]
                    } else { 0.0 };

                    // Fetch the NEW source directly for the whole window: in
                    // the first half output[] still holds the previous source,
                    // so blending against output would make the ramp a no-op
                    // until the midpoint and leave a half-amplitude step.
                    let new_offset = offsets[best_src];
                    let new_idx = (fade_start + j) as i64 - new_offset;
                    let new_sample = if new_idx >= 0 && (new_idx as usize) < sources[best_src].len() {
                        sources[best_src][new_idx as usize]
                    } else { 0.0 };

                    output[fade_start + j] = prev_sample * (1.0 - t) + new_sample * t;
                }
            }
        }

        prev_best = Some(best_src);
        pos += segment_size;
    }

    output
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn decode_to_mono(app: &AppHandle, path: &str, rate: u32) -> Result<PathBuf, String> {
    let tmp = std::env::temp_dir().join(format!(
        "depoaudio_merge_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));

    let args: Vec<String> = vec![
        "-i".into(), path.to_string(),
        "-af".into(), format!("aresample={}", rate),
        "-ac".into(), "1".into(),
        "-acodec".into(), "pcm_f32le".into(),
        "-y".into(), tmp.to_string_lossy().to_string(),
    ];

    let output = app.shell()
        .sidecar(ffmpeg_bin_name())
        .map_err(|e| e.to_string())?
        .args(args)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if output.status.code() != Some(0) {
        let _ = std::fs::remove_file(&tmp);
        return Err("Failed to decode audio for merge".into());
    }

    Ok(tmp)
}

fn read_wav_samples(path: &Path) -> Result<Vec<f32>, String> {
    let reader = hound::WavReader::open(path)
        .map_err(|e| format!("WAV read error: {}", e))?;
    Ok(reader.into_samples::<f32>().filter_map(|s| s.ok()).collect())
}

fn write_wav(path: &Path, samples: &[f32], rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)
        .map_err(|e| format!("WAV write error: {}", e))?;
    for &s in samples {
        writer.write_sample(s).map_err(|e| format!("Write error: {}", e))?;
    }
    writer.finalize().map_err(|e| format!("Finalize error: {}", e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_correlate_offset_sign_convention() {
        // Construct B containing A's content delayed by 100 samples:
        // b[i + 100] = a[i], i.e. recorder B started 100 samples EARLIER.
        let mut a = vec![0.0f32; 400];
        a[10] = 1.0;
        a[50] = -0.5;
        a[200] = 0.8;
        let mut b = vec![0.0f32; 600];
        for (i, &v) in a.iter().enumerate() {
            b[i + 100] = v;
        }

        // The correlation peak must be at offset = +100 (a[i] vs b[i + offset]),
        // which detect_sync negates so that positive offset_seconds means
        // "source B starts later".
        let at_plus = cross_correlate(&a, &b, 100, a.len());
        let at_zero = cross_correlate(&a, &b, 0, a.len());
        let at_minus = cross_correlate(&a, &b, -100, a.len());
        assert!(at_plus > at_zero, "peak should be at +100, not 0");
        assert!(at_plus > at_minus, "peak should be at +100, not -100");
    }
}
