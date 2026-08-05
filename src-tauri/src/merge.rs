use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use tauri::AppHandle;
use uuid::Uuid;

use crate::ffmpeg::{probe_duration, sidecar_output_opt};
use crate::helpers::ffmpeg_bin_name;
use crate::safety::TempFile;

const MAX_MERGE_SOURCES: usize = 4;
const MAX_MERGE_SOURCE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MERGE_TOTAL_SOURCE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The current merge strategies hold all decoded sources plus an output
/// timeline (and, for mix-all, a one-byte-per-sample overlap counter). At nine
/// bytes per bounded sample in the worst case, 56 million keeps peak working
/// memory below roughly 512 MiB.
const MAX_TOTAL_DECODED_SAMPLES: u64 = 56_000_000;
const MAX_MERGE_DURATION_SECS: u32 = 30 * 60;
const SYNC_SAMPLE_RATE: u32 = 8_000;
const SYNC_WINDOW_SECS: usize = 10;
const SYNC_SEARCH_SECS: usize = 60;
const SYNC_DECODE_SECS: u32 = 75;
const MERGE_DECODE_TIMEOUT_SECS: u64 = 20 * 60;
const MERGE_ENCODE_TIMEOUT_SECS: u64 = 20 * 60;

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

struct ValidatedMergeJob {
    sources: Vec<PathBuf>,
    out_dir: PathBuf,
    out_name: String,
    format: String,
    rate: String,
    strategy: String,
}

fn validate_source(path: &str) -> Result<(PathBuf, u64), String> {
    if path.trim().is_empty() {
        return Err("Audio source path is empty".into());
    }
    let canonical = std::fs::canonicalize(Path::new(path)).map_err(|_| "Audio source was not found".to_string())?;
    crate::safety::check_file_safe_with_limit(&canonical, MAX_MERGE_SOURCE_BYTES)?;
    let size = std::fs::metadata(&canonical)
        .map_err(|_| "Cannot inspect audio source".to_string())?
        .len();
    Ok((canonical, size))
}

fn validate_output_name(name: &str) -> Result<String, String> {
    let name = if name.trim().is_empty() { "merged" } else { name.trim() };
    if name.len() > 128 || name.contains(['/', '\\', ':']) || name.chars().any(char::is_control) {
        return Err("Output name must be one filename of at most 128 characters".into());
    }
    let mut components = Path::new(name).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) if component == std::ffi::OsStr::new(name) => Ok(name.to_string()),
        _ => Err("Output name must be one filename".into()),
    }
}

fn validate_merge_job(job: &MergeJob) -> Result<ValidatedMergeJob, String> {
    if !(2..=MAX_MERGE_SOURCES).contains(&job.sources.len()) {
        return Err(format!("Merge requires 2 to {MAX_MERGE_SOURCES} source files"));
    }
    if !matches!(job.format.as_str(), "wav" | "mp3" | "flac" | "opus") {
        return Err("Unsupported merge output format".into());
    }
    if !matches!(job.rate.as_str(), "44100" | "48000") {
        return Err("Merge sample rate must be 44100 or 48000 Hz".into());
    }
    if !matches!(job.strategy.as_str(), "best_quality" | "mix_all") {
        return Err("Unsupported merge strategy".into());
    }

    let mut sources = Vec::with_capacity(job.sources.len());
    let mut seen = HashSet::with_capacity(job.sources.len());
    let mut total_size = 0u64;
    for source in &job.sources {
        let (canonical, size) = validate_source(source)?;
        if !seen.insert(canonical.clone()) {
            return Err("The same source file cannot be merged twice".into());
        }
        total_size = total_size
            .checked_add(size)
            .ok_or_else(|| "Combined source size is too large".to_string())?;
        if total_size > MAX_MERGE_TOTAL_SOURCE_BYTES {
            return Err("Combined merge sources exceed the 4 GB safety limit".into());
        }
        sources.push(canonical);
    }

    let out_dir = if job.out_dir.trim().is_empty() {
        sources[0]
            .parent()
            .ok_or_else(|| "Cannot determine merge output directory".to_string())?
            .to_path_buf()
    } else {
        std::fs::canonicalize(Path::new(&job.out_dir))
            .map_err(|_| "Merge output directory was not found".to_string())?
    };
    if !out_dir.is_dir() {
        return Err("Merge output path is not a directory".into());
    }

    Ok(ValidatedMergeJob {
        sources,
        out_dir,
        out_name: validate_output_name(&job.out_name)?,
        format: job.format.clone(),
        rate: job.rate.clone(),
        strategy: job.strategy.clone(),
    })
}

/// Strip an SGMCA vendor prefix once and keep the temporary Ogg feed alive for
/// the whole merge. Reusing it avoids copying a potentially large recording
/// for duration probing, sync detection, and final decoding independently.
async fn prepare_merge_source(path: PathBuf) -> Result<(PathBuf, Option<TempFile>), String> {
    crate::helpers::prepare_audio_feed_cancellable(path, Arc::new(|| false)).await
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
    pub warning: Option<String>,
}

// ── Sync detection ──────────────────────────────────────────────────────────

/// Detect the timing offset between two audio files using cross-correlation.
/// Returns the offset in seconds that aligns source_b to source_a.
pub(crate) async fn detect_sync(app: &AppHandle, source_a: &str, source_b: &str) -> Result<SyncResult, String> {
    let (source_a, _) = validate_source(source_a)?;
    let (source_b, _) = validate_source(source_b)?;
    if source_a == source_b {
        return Err("Choose two different source files for sync detection".into());
    }
    let (source_a, _guard_a) = prepare_merge_source(source_a).await?;
    let (source_b, _guard_b) = prepare_merge_source(source_b).await?;
    detect_sync_paths(app, &source_a, &source_b).await
}

async fn detect_sync_paths(app: &AppHandle, source_a: &Path, source_b: &Path) -> Result<SyncResult, String> {
    // A short, low-rate decode contains enough speech for sync detection while
    // bounding temporary disk use and correlation work.
    let tmp_a = decode_to_mono(app, source_a, SYNC_SAMPLE_RATE, SYNC_DECODE_SECS).await?;
    let tmp_b = decode_to_mono(app, source_b, SYNC_SAMPLE_RATE, SYNC_DECODE_SECS).await?;
    let max_sync_samples = SYNC_SAMPLE_RATE as usize * SYNC_DECODE_SECS as usize + 1;
    let samples_a = read_wav_samples(&tmp_a, max_sync_samples)?;
    let samples_b = read_wav_samples(&tmp_b, max_sync_samples)?;

    tauri::async_runtime::spawn_blocking(move || detect_sync_samples(&samples_a, &samples_b, SYNC_SAMPLE_RATE as usize))
        .await
        .map_err(|e| format!("Sync analysis task failed: {e}"))?
}

fn detect_sync_samples(samples_a: &[f32], samples_b: &[f32], sample_rate: usize) -> Result<SyncResult, String> {
    if samples_a.is_empty() || samples_b.is_empty() {
        return Err("One or both files are empty".into());
    }

    let search_len = (SYNC_WINDOW_SECS * sample_rate)
        .min(samples_a.len())
        .min(samples_b.len());
    let max_offset = SYNC_SEARCH_SECS * sample_rate;

    let segment_a = &samples_a[..search_len];

    let mut best_corr = 0.0f64;
    let mut best_offset = 0i64;

    // Coarse search: 100 ms steps. Sampling every fourth point bounds CPU
    // without changing the time-offset search range.
    let coarse_stride = (sample_rate / 10).max(1);
    let search_range = max_offset.min(samples_b.len());

    let neg_range = -(search_range as i64);
    let pos_range = search_range as i64;
    let mut coarse_offset = neg_range;
    while coarse_offset < pos_range {
        let offset = coarse_offset;
        coarse_offset += coarse_stride as i64;
        let corr = cross_correlate_step(segment_a, samples_b, offset, search_len, 4);
        if corr > best_corr {
            best_corr = corr;
            best_offset = offset;
        }
    }

    // Fine search: 1 ms steps around the best coarse offset.
    let fine_start = best_offset - coarse_stride as i64;
    let fine_end = best_offset + coarse_stride as i64;
    let fine_stride = (sample_rate / 1000).max(1);
    for offset in (fine_start..=fine_end).step_by(fine_stride) {
        let corr = cross_correlate_step(segment_a, samples_b, offset, search_len, 4);
        if corr > best_corr {
            best_corr = corr;
            best_offset = offset;
        }
    }

    // Resolve the final 1 ms neighborhood at full sample precision.
    let sample_start = best_offset - fine_stride as i64;
    let sample_end = best_offset + fine_stride as i64;
    for offset in sample_start..=sample_end {
        let corr = cross_correlate(segment_a, samples_b, offset, search_len);
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
        (best_corr / (energy_a.sqrt() * energy_b.sqrt())).clamp(0.0, 1.0)
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
    cross_correlate_step(segment_a, samples_b, offset, len, 1)
}

fn cross_correlate_step(segment_a: &[f32], samples_b: &[f32], offset: i64, len: usize, step: usize) -> f64 {
    let mut sum = 0.0f64;
    let len = len.min(segment_a.len());
    for i in (0..len).step_by(step.max(1)) {
        let a = segment_a[i];
        let b_idx = i as i64 + offset;
        if b_idx >= 0 && (b_idx as usize) < samples_b.len() {
            sum += a as f64 * samples_b[b_idx as usize] as f64;
        }
    }
    sum.abs()
}

// ── Merge execution ─────────────────────────────────────────────────────────

/// Merge multiple audio files into one synchronized output.
pub(crate) async fn merge_audio(app: &AppHandle, job: &MergeJob) -> Result<MergeResult, String> {
    let mut job = validate_merge_job(job)?;
    let mut prepared_sources = Vec::with_capacity(job.sources.len());
    let mut source_guards = Vec::new();
    for source in &job.sources {
        let (prepared, guard) = prepare_merge_source(source.clone()).await?;
        prepared_sources.push(prepared);
        if let Some(guard) = guard {
            source_guards.push(guard);
        }
    }
    job.sources = prepared_sources;
    // The guards intentionally live until every probe/decode finishes.
    let _source_guards = source_guards;

    // Probe before decoding so a very long compressed source cannot fill the
    // temp directory or exhaust memory before the decoded-size limit is seen.
    let mut estimated_samples = 0f64;
    for source in &job.sources {
        let duration = probe_duration(app, source)
            .await
            .filter(|duration| duration.is_finite() && *duration > 0.0)
            .ok_or_else(|| "Cannot determine merge source duration".to_string())?;
        if duration > MAX_MERGE_DURATION_SECS as f64 {
            return Err(format!(
                "Each merge source is limited to {} minutes",
                MAX_MERGE_DURATION_SECS / 60
            ));
        }
        estimated_samples += duration * 48_000.0;
        if estimated_samples > MAX_TOTAL_DECODED_SAMPLES as f64 {
            return Err("Combined decoded audio is too long for the bounded in-memory merge engine".into());
        }
    }

    // Step 1: Detect sync offsets relative to the first source
    let mut offsets = vec![0.0f64]; // First source is reference (offset = 0)
    for i in 1..job.sources.len() {
        let sync = detect_sync_paths(app, &job.sources[0], &job.sources[i]).await?;
        if !sync.is_same_event {
            return Err(format!(
                "Source {} could not be confidently matched to the reference recording",
                i + 1
            ));
        }
        offsets.push(sync.offset_seconds);
    }

    // Step 2: Decode all sources to 48kHz mono
    let mut decoded_files = Vec::new();
    for src in &job.sources {
        let tmp = decode_to_mono(app, src, 48_000, MAX_MERGE_DURATION_SECS + 1).await?;
        decoded_files.push(tmp);
    }

    // Step 3: Validate aggregate decoded allocation before reading any source.
    let mut decoded_sample_count = 0u64;
    for file in &decoded_files {
        decoded_sample_count = decoded_sample_count
            .checked_add(wav_sample_count(file)?)
            .ok_or_else(|| "Decoded merge size overflow".to_string())?;
        if decoded_sample_count > MAX_TOTAL_DECODED_SAMPLES {
            return Err("Combined decoded audio exceeds the merge memory limit".into());
        }
    }
    let decoded_paths: Vec<PathBuf> = decoded_files.iter().map(|file| file.path.clone()).collect();
    let all_samples = tauri::async_runtime::spawn_blocking(move || {
        decoded_paths
            .iter()
            .map(|path| read_wav_samples(path, MAX_TOTAL_DECODED_SAMPLES as usize))
            .collect::<Result<Vec<_>, _>>()
    })
    .await
    .map_err(|e| format!("Decoded audio read task failed: {e}"))??;
    drop(decoded_files);

    // Step 4: Align to common timeline
    let sample_rate = 48_000usize;
    let min_offset = offsets.iter().cloned().fold(f64::INFINITY, f64::min);
    let adjusted_offsets: Vec<i64> = offsets
        .iter()
        .map(|&o| ((o - min_offset) * sample_rate as f64) as i64)
        .collect();

    // Find total duration (longest aligned track)
    let total_samples = all_samples
        .iter()
        .enumerate()
        .map(|(i, samples)| {
            usize::try_from(adjusted_offsets[i])
                .ok()
                .and_then(|offset| samples.len().checked_add(offset))
                .ok_or_else(|| "Aligned merge timeline is too large".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    if total_samples == 0 || total_samples as u64 > MAX_TOTAL_DECODED_SAMPLES {
        return Err("Aligned merge timeline exceeds the merge memory limit".into());
    }

    // Step 5: Build and write the intermediate on a blocking worker. The
    // merged Vec is dropped there before encoding starts.
    let tmp_wav_path = std::env::temp_dir().join(format!(
        "depoaudio_merged_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));
    let tmp_wav = TempFile::new(tmp_wav_path.clone());
    let strategy = job.strategy.clone();
    let duration = tauri::async_runtime::spawn_blocking(move || -> Result<f64, String> {
        let merged = match strategy.as_str() {
            "mix_all" => mix_all_strategy(&all_samples, &adjusted_offsets, total_samples),
            "best_quality" => best_quality_strategy(&all_samples, &adjusted_offsets, total_samples, sample_rate),
            _ => return Err("Unsupported merge strategy".into()),
        };
        write_wav(&tmp_wav_path, &merged, 48_000)?;
        Ok(merged.len() as f64 / 48_000.0)
    })
    .await
    .map_err(|e| format!("Merge processing task failed: {e}"))??;

    // Step 6: Reserve the destination atomically, then encode with a bounded
    // child-process runner. No concurrent merge can claim or delete this path.
    let ext = crate::helpers::output_ext(&job.format);
    let mut reserved = reserve_output(&job.out_dir, &job.out_name, ext)?;
    let out_path = reserved.path.clone();
    let out_codec = crate::helpers::output_args(&job.format, &job.rate, 192);
    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-i".into(),
        tmp_wav.to_string_lossy().to_string(),
    ];
    args.extend(out_codec);
    args.extend(["-y".into(), out_path.to_string_lossy().to_string()]);
    let output = sidecar_output_opt(app, ffmpeg_bin_name(), args, MERGE_ENCODE_TIMEOUT_SECS)
        .await
        .ok_or_else(|| "Merged output encoder timed out or could not start".to_string())?;
    if !output.success {
        return Err("Failed to encode merged output".into());
    }

    let size = std::fs::metadata(&out_path)
        .map_err(|_| "Merged output was not created".to_string())?
        .len();
    if size == 0 {
        return Err("Merged output is empty".into());
    }
    reserved.keep = true;

    Ok(MergeResult {
        output_path: out_path.to_string_lossy().to_string(),
        output_name: crate::helpers::basename(&out_path.to_string_lossy()),
        output_size: size,
        duration,
        sources_used: job.sources.len(),
        sync_offsets: offsets,
        warning: None,
    })
}

// ── Merge strategies ────────────────────────────────────────────────────────

/// Mix all sources together with equal weight (simple average).
fn mix_all_strategy(sources: &[Vec<f32>], offsets: &[i64], total_samples: usize) -> Vec<f32> {
    let mut output = vec![0.0f32; total_samples];
    // Source count is capped at four, so one byte per sample is sufficient and
    // avoids another four-byte timeline-sized allocation.
    let mut counts = vec![0u8; total_samples];

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
fn best_quality_strategy(sources: &[Vec<f32>], offsets: &[i64], total_samples: usize, sample_rate: usize) -> Vec<f32> {
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

            let rms = if count > 0 {
                (rms_sum / count as f64).sqrt()
            } else {
                0.0
            };
            if rms > best_rms {
                best_rms = rms;
                best_src = src_idx;
            }
        }

        // Copy best source to output
        let offset = offsets[best_src];
        for (j, out) in output[pos..end].iter_mut().enumerate() {
            let src_idx_sample = (pos + j) as i64 - offset;
            if src_idx_sample >= 0 && (src_idx_sample as usize) < sources[best_src].len() {
                *out = sources[best_src][src_idx_sample as usize];
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
                    } else {
                        0.0
                    };

                    // Fetch the NEW source directly for the whole window: in
                    // the first half output[] still holds the previous source,
                    // so blending against output would make the ramp a no-op
                    // until the midpoint and leave a half-amplitude step.
                    let new_offset = offsets[best_src];
                    let new_idx = (fade_start + j) as i64 - new_offset;
                    let new_sample = if new_idx >= 0 && (new_idx as usize) < sources[best_src].len() {
                        sources[best_src][new_idx as usize]
                    } else {
                        0.0
                    };

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

async fn decode_to_mono(app: &AppHandle, path: &Path, rate: u32, max_secs: u32) -> Result<TempFile, String> {
    let tmp_path = std::env::temp_dir().join(format!(
        "depoaudio_merge_{}.wav",
        Uuid::new_v4().to_string().replace('-', "")
    ));
    let tmp = TempFile::new(tmp_path);

    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file".into(),
    ];
    let input_codec = crate::helpers::input_codec_args(path);
    if !input_codec.is_empty() {
        crate::ffmpeg::ensure_ftr_decoder(app).await?;
        args.extend(input_codec);
    }
    args.extend([
        "-i".into(),
        path.to_string_lossy().to_string(),
        "-t".into(),
        max_secs.to_string(),
        "-vn".into(),
        "-sn".into(),
        "-dn".into(),
        "-af".into(),
        format!("aresample={}", rate),
        "-ac".into(),
        "1".into(),
        "-acodec".into(),
        "pcm_f32le".into(),
        "-y".into(),
        tmp.to_string_lossy().to_string(),
    ]);

    let output = sidecar_output_opt(app, ffmpeg_bin_name(), args, MERGE_DECODE_TIMEOUT_SECS)
        .await
        .ok_or_else(|| "Merge decoder timed out or could not start".to_string())?;
    if !output.success {
        return Err("Failed to decode audio for merge".into());
    }

    Ok(tmp)
}

fn wav_sample_count(path: &Path) -> Result<u64, String> {
    let reader = hound::WavReader::open(path).map_err(|e| format!("WAV read error: {}", e))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.bits_per_sample != 32 || spec.sample_format != hound::SampleFormat::Float {
        return Err("Decoded merge audio has an unexpected WAV layout".into());
    }
    Ok(reader.duration() as u64)
}

fn read_wav_samples(path: &Path, max_samples: usize) -> Result<Vec<f32>, String> {
    let reader = hound::WavReader::open(path).map_err(|e| format!("WAV read error: {}", e))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.bits_per_sample != 32 || spec.sample_format != hound::SampleFormat::Float {
        return Err("Decoded merge audio has an unexpected WAV layout".into());
    }
    let count = reader.duration() as usize;
    if count > max_samples {
        return Err("Decoded merge audio exceeds the sample limit".into());
    }
    reader
        .into_samples::<f32>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("WAV sample decode error: {e}"))
}

fn write_wav(path: &Path, samples: &[f32], rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).map_err(|e| format!("WAV write error: {}", e))?;
    for &s in samples {
        writer.write_sample(s).map_err(|e| format!("Write error: {}", e))?;
    }
    writer.finalize().map_err(|e| format!("Finalize error: {}", e))?;
    Ok(())
}

struct ReservedOutput {
    path: PathBuf,
    keep: bool,
}

impl Drop for ReservedOutput {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn reserve_output(out_dir: &Path, name: &str, ext: &str) -> Result<ReservedOutput, String> {
    let requested = out_dir.join(format!("{name}{ext}"));
    let path = crate::helpers::reserve_unique_path(&requested)?;
    Ok(ReservedOutput { path, keep: false })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("depoaudio_merge_{label}_{}", Uuid::new_v4().simple()))
    }

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

    #[test]
    fn output_name_rejects_path_traversal_and_absolute_forms() {
        for name in [
            "..",
            "../outside",
            "..\\outside",
            "/tmp/out",
            "C:\\temp\\out",
            "out.wav:stream",
        ] {
            assert!(validate_output_name(name).is_err(), "accepted {name:?}");
        }
        assert_eq!(validate_output_name("  merged session  ").unwrap(), "merged session");
    }

    #[test]
    fn duplicate_sources_are_rejected_after_canonicalization() {
        let dir = test_dir("duplicates");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("recording.wav");
        std::fs::write(&source, b"not empty").unwrap();
        let source = source.to_string_lossy().to_string();
        let job = MergeJob {
            sources: vec![source.clone(), source],
            out_dir: String::new(),
            out_name: "merged".into(),
            format: "wav".into(),
            rate: "48000".into(),
            strategy: "mix_all".into(),
        };
        assert!(validate_merge_job(&job).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn output_reservation_is_no_clobber_and_cleans_failed_candidate() {
        let dir = test_dir("reserve");
        std::fs::create_dir_all(&dir).unwrap();
        let existing = dir.join("merged.wav");
        std::fs::write(&existing, b"keep me").unwrap();
        let reserved_path;
        {
            let reserved = reserve_output(&dir, "merged", ".wav").unwrap();
            reserved_path = reserved.path.clone();
            assert_ne!(reserved.path, existing);
            assert!(reserved.path.exists());
        }
        assert!(!reserved_path.exists());
        assert_eq!(std::fs::read(existing).unwrap(), b"keep me");
        let _ = std::fs::remove_dir_all(dir);
    }
}
