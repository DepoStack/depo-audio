use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::AppHandle;

use crate::ffmpeg::{probe_duration_cancellable, sidecar_output_cancellable};
use crate::helpers::{ffmpeg_bin_name, AudioFeed};
use crate::merge_engine::{self, AlignedSource, MergeStrategy, SampleReader, MAX_MERGE_DURATION_SECS};
use crate::safety::TempFile;

const MAX_MERGE_SOURCES: usize = 4;
const MAX_MERGE_SOURCE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_MERGE_TOTAL_SOURCE_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The merge itself decodes and mixes at this rate; the output rate is the
/// encoder's job.
const MERGE_SAMPLE_RATE: u32 = 48_000;
const SYNC_SAMPLE_RATE: u32 = 8_000;
const SYNC_DECODE_SECS: u32 = 75;
const MERGE_DECODE_TIMEOUT_SECS: u64 = 20 * 60;
const MERGE_ENCODE_TIMEOUT_SECS: u64 = 20 * 60;

/// Returned when a merge or sync detection stops because it was cancelled.
pub(crate) const MERGE_CANCELLED_MESSAGE: &str = "Merge cancelled";

/// Cancel flag for one merge or sync detection. Clones share the flag: the
/// command that starts a merge keeps one, and a cancel command sets it. Every
/// step checks it, the source copies stop between 64 KiB chunks, and a
/// running FFmpeg or ffprobe is killed within about a second.
#[derive(Clone, Debug, Default)]
pub(crate) struct MergeCancel(Arc<AtomicBool>);

impl MergeCancel {
    pub(crate) fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn check(&self) -> Result<(), String> {
        if self.is_cancelled() {
            Err(MERGE_CANCELLED_MESSAGE.into())
        } else {
            Ok(())
        }
    }

    /// The predicate the FFmpeg runners and the feed preparation take.
    fn predicate(&self) -> Arc<dyn Fn() -> bool + Send + Sync> {
        let flag = self.0.clone();
        Arc::new(move || flag.load(Ordering::Relaxed))
    }
}

// ── Multi-source audio merge ────────────────────────────────────────────────
//
// Combines multiple recordings of the same event into one clean output.
// Typical use: court reporter mic + backup recorder + phone-in participant.
//
// Pipeline:
//   1. Probe every source and refuse one over 30 minutes, by name
//   2. Find each source's offset from the first by cross-correlation
//   3. Decode every source to a temporary 48 kHz mono WAV
//   4. Stream the aligned sources through merge_engine block by block,
//      averaging them (Mix all) or taking the loudest per 500 ms segment
//      with a 50 ms crossfade (Loudest source), into one temporary WAV
//   5. Encode that WAV to the chosen format and rate

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
    strategy: MergeStrategy,
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
    let strategy = match job.strategy.as_str() {
        "mix_all" => MergeStrategy::MixAll,
        "best_quality" => MergeStrategy::Loudest,
        _ => return Err("Unsupported merge strategy".into()),
    };

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
        strategy,
    })
}

/// Read an SGMCA's layout (or a `.dm`'s blocks, or a `.dcr`, or split an
/// FTR recording's channels) once and keep its temporary feed alive for the
/// whole merge. Reusing it avoids reading every page of a potentially large
/// recording again for duration probing, sync detection, and decoding for
/// the merge.
async fn prepare_merge_source(app: &AppHandle, path: PathBuf, cancel: &MergeCancel) -> Result<AudioFeed, String> {
    cancel.check()?;
    let options = crate::ffmpeg::feed_options(app, &path).await;
    let feed = prepare_merge_feed(path, options, cancel).await?;
    // A DCR whose channels FFmpeg still has to join.
    let finished = crate::helpers::finish_prepared_feed(app, feed, &|| cancel.is_cancelled()).await;
    cancel.check()?;
    finished
}

async fn prepare_merge_feed(
    path: PathBuf,
    options: crate::helpers::FeedOptions,
    cancel: &MergeCancel,
) -> Result<AudioFeed, String> {
    cancel.check()?;
    let prepared = crate::helpers::prepare_audio_feed_with_options(path, options, cancel.predicate()).await;
    cancel.check()?;
    prepared
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
pub(crate) async fn detect_sync(
    app: &AppHandle,
    source_a: &str,
    source_b: &str,
    cancel: &MergeCancel,
) -> Result<SyncResult, String> {
    let (source_a, _) = validate_source(source_a)?;
    let (source_b, _) = validate_source(source_b)?;
    if source_a == source_b {
        return Err("Choose two different source files for sync detection".into());
    }
    let mut source_a = prepare_merge_source(app, source_a, cancel).await?;
    let mut source_b = prepare_merge_source(app, source_b, cancel).await?;
    detect_sync_paths(app, &mut source_a, &mut source_b, cancel).await
}

async fn detect_sync_paths(
    app: &AppHandle,
    source_a: &mut AudioFeed,
    source_b: &mut AudioFeed,
    cancel: &MergeCancel,
) -> Result<SyncResult, String> {
    // A short, low-rate decode contains enough speech for sync detection while
    // bounding temporary disk use and correlation work.
    let tmp_a = decode_to_mono(app, source_a, SYNC_SAMPLE_RATE, SYNC_DECODE_SECS, cancel).await?;
    let tmp_b = decode_to_mono(app, source_b, SYNC_SAMPLE_RATE, SYNC_DECODE_SECS, cancel).await?;
    let max_sync_samples = SYNC_SAMPLE_RATE as usize * SYNC_DECODE_SECS as usize + 1;
    let samples_a = read_wav_samples(&tmp_a, max_sync_samples)?;
    let samples_b = read_wav_samples(&tmp_b, max_sync_samples)?;

    let found = tauri::async_runtime::spawn_blocking(move || {
        merge_engine::detect_sync_samples(&samples_a, &samples_b, SYNC_SAMPLE_RATE as usize)
    })
    .await
    .map_err(|e| format!("Sync analysis task failed: {e}"))??;
    Ok(SyncResult {
        offset_seconds: found.offset_seconds,
        confidence: found.confidence,
        is_same_event: found.is_same_event,
    })
}

// ── Merge execution ─────────────────────────────────────────────────────────

/// Merge multiple audio files into one synchronized output.
pub(crate) async fn merge_audio(app: &AppHandle, job: &MergeJob, cancel: &MergeCancel) -> Result<MergeResult, String> {
    let job = validate_merge_job(job)?;
    // Each feed (and any temporary file it owns) lives until every
    // probe/decode finishes.
    let mut feeds = Vec::with_capacity(job.sources.len());
    for source in &job.sources {
        feeds.push(prepare_merge_source(app, source.clone(), cancel).await?);
    }

    // Probe before decoding so a very long compressed source cannot fill the
    // temp directory before its length is known.
    for (source, path) in feeds.iter().zip(&job.sources) {
        let duration = probe_duration_cancellable(app, source, Some(&|| cancel.is_cancelled())).await;
        cancel.check()?;
        merge_engine::check_source_duration(&crate::helpers::basename(&path.to_string_lossy()), duration)?;
    }

    // Step 1: Detect sync offsets relative to the first source
    let mut offsets = vec![0.0f64]; // First source is reference (offset = 0)
    for i in 1..feeds.len() {
        let (reference, rest) = feeds.split_at_mut(1);
        let sync = detect_sync_paths(app, &mut reference[0], &mut rest[i - 1], cancel).await?;
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
    for src in &mut feeds {
        let tmp = decode_to_mono(app, src, MERGE_SAMPLE_RATE, MAX_MERGE_DURATION_SECS + 1, cancel).await?;
        decoded_files.push(tmp);
    }
    drop(feeds);

    // Step 3: Place each source on a common timeline, the earliest at zero.
    let rate = MERGE_SAMPLE_RATE as f64;
    let min_offset = offsets.iter().cloned().fold(f64::INFINITY, f64::min);
    let mut sources = Vec::with_capacity(decoded_files.len());
    for (file, &offset) in decoded_files.iter().zip(&offsets) {
        let reader = WavSampleReader::open(&file.path)?;
        let len = reader.len;
        sources.push(AlignedSource::new(reader, ((offset - min_offset) * rate) as u64, len));
    }
    let total_samples = merge_engine::timeline_len(&sources);
    if total_samples == 0 {
        return Err("The merge sources decoded to no audio".into());
    }
    if total_samples > merge_engine::max_timeline_samples(MERGE_SAMPLE_RATE) {
        return Err("The aligned recordings run longer than a merge can write".into());
    }

    // Step 4: Stream the merge into an intermediate WAV on a blocking worker.
    let tmp_wav = crate::workdir::new_temp_path("depoaudio_merged_", "wav")?;
    let tmp_wav_path = tmp_wav.path.clone();
    let strategy = job.strategy;
    let worker_cancel = cancel.clone();
    let written = tauri::async_runtime::spawn_blocking(move || -> Result<u64, String> {
        let mut writer = wav_writer(&tmp_wav_path, MERGE_SAMPLE_RATE)?;
        let mut sink = |block: &[f32]| {
            for &sample in block {
                writer.write_sample(sample).map_err(|e| format!("Write error: {e}"))?;
            }
            Ok(())
        };
        let mut tick = |_done: u64, _total: u64| worker_cancel.check();
        let written =
            merge_engine::merge_stream(strategy, &mut sources, MERGE_SAMPLE_RATE as usize, &mut sink, &mut tick)?;
        writer.finalize().map_err(|e| format!("Finalize error: {e}"))?;
        Ok(written)
    })
    .await
    .map_err(|e| format!("Merge processing task failed: {e}"))??;
    drop(decoded_files);
    cancel.check()?;
    let duration = written as f64 / rate;

    // Step 5: Encode.
    // Reserve the destination atomically, then encode with a bounded
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
    let output = sidecar_output_cancellable(
        app,
        ffmpeg_bin_name(),
        args,
        MERGE_ENCODE_TIMEOUT_SECS,
        Some(&|| cancel.is_cancelled()),
    )
    .await;
    // A cancelled encode leaves the reserved output to be removed on drop.
    cancel.check()?;
    let output = output.ok_or_else(|| "Merged output encoder timed out or could not start".to_string())?;
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

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Decode `feed` to mono. If FFmpeg's native FTR decoder fails, the feed is
/// replaced by the split-channel route and decoded once more, as conversion
/// and Scan do; later passes then use the split route too.
async fn decode_to_mono(
    app: &AppHandle,
    feed: &mut AudioFeed,
    rate: u32,
    max_secs: u32,
    cancel: &MergeCancel,
) -> Result<TempFile, String> {
    match decode_feed_to_mono(app, feed, rate, max_secs, cancel).await {
        Err(error) if feed.used_native_ftr() && !cancel.is_cancelled() => {
            let source = feed.path().to_path_buf();
            *feed = crate::helpers::prepare_audio_feed_after_native_failure(app, source, cancel.predicate())
                .await
                .map_err(|split| {
                    if cancel.is_cancelled() {
                        MERGE_CANCELLED_MESSAGE.to_string()
                    } else {
                        format!("{error}; splitting its channels also failed: {split}")
                    }
                })?;
            decode_feed_to_mono(app, feed, rate, max_secs, cancel).await
        }
        other => other,
    }
}

async fn decode_feed_to_mono(
    app: &AppHandle,
    feed: &AudioFeed,
    rate: u32,
    max_secs: u32,
    cancel: &MergeCancel,
) -> Result<TempFile, String> {
    cancel.check()?;
    let tmp = crate::workdir::new_temp_path("depoaudio_merge_", "wav")?;

    let mut args: Vec<String> = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file".into(),
    ];
    args.extend(feed.input_args(&[]));
    // The same audio stream every other pass reads, not FFmpeg's pick.
    args.extend(feed.source_output_args(Some(&format!("aresample={rate}"))));
    args.extend([
        "-t".into(),
        max_secs.to_string(),
        "-vn".into(),
        "-sn".into(),
        "-dn".into(),
        "-ac".into(),
        "1".into(),
        "-acodec".into(),
        "pcm_f32le".into(),
        "-y".into(),
        tmp.to_string_lossy().to_string(),
    ]);

    let output = sidecar_output_cancellable(
        app,
        ffmpeg_bin_name(),
        args,
        MERGE_DECODE_TIMEOUT_SECS,
        Some(&|| cancel.is_cancelled()),
    )
    .await;
    cancel.check()?;
    let output = output.ok_or_else(|| "Merge decoder timed out or could not start".to_string())?;
    if !output.success {
        return Err("Failed to decode audio for merge".into());
    }

    Ok(tmp)
}

fn open_float_wav(path: &Path) -> Result<hound::WavReader<std::io::BufReader<std::fs::File>>, String> {
    let reader = hound::WavReader::open(path).map_err(|e| format!("WAV read error: {}", e))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.bits_per_sample != 32 || spec.sample_format != hound::SampleFormat::Float {
        return Err("Decoded merge audio has an unexpected WAV layout".into());
    }
    Ok(reader)
}

/// Reads one decoded source front to back for the merge engine.
struct WavSampleReader {
    reader: hound::WavReader<std::io::BufReader<std::fs::File>>,
    len: u64,
}

impl WavSampleReader {
    fn open(path: &Path) -> Result<Self, String> {
        let reader = open_float_wav(path)?;
        let len = reader.duration() as u64;
        Ok(Self { reader, len })
    }
}

impl SampleReader for WavSampleReader {
    fn read_exact_into(&mut self, out: &mut [f32]) -> Result<(), String> {
        let mut samples = self.reader.samples::<f32>();
        for slot in out.iter_mut() {
            *slot = samples
                .next()
                .ok_or_else(|| "Decoded merge audio ended early".to_string())?
                .map_err(|e| format!("WAV sample decode error: {e}"))?;
        }
        Ok(())
    }
}

fn read_wav_samples(path: &Path, max_samples: usize) -> Result<Vec<f32>, String> {
    let reader = open_float_wav(path)?;
    let count = reader.duration() as usize;
    if count > max_samples {
        return Err("Decoded merge audio exceeds the sample limit".into());
    }
    reader
        .into_samples::<f32>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("WAV sample decode error: {e}"))
}

fn wav_writer(path: &Path, rate: u32) -> Result<hound::WavWriter<std::io::BufWriter<std::fs::File>>, String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    hound::WavWriter::create(path, spec).map_err(|e| format!("WAV write error: {}", e))
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
    use uuid::Uuid;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("depoaudio_merge_{label}_{}", Uuid::new_v4().simple()))
    }

    #[test]
    fn decoded_wavs_stream_through_the_engine_into_a_wav() {
        // Two decoded sources as the merge writes them, the second placed
        // three samples later: the engine reads them in blocks and the
        // intermediate WAV holds their average where both play.
        let dir = test_dir("engine_wav");
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, samples: &[f32]| {
            let path = dir.join(name);
            let mut writer = wav_writer(&path, MERGE_SAMPLE_RATE).unwrap();
            for &s in samples {
                writer.write_sample(s).unwrap();
            }
            writer.finalize().unwrap();
            path
        };
        let a = write("a.wav", &[1.0; 8]);
        let b = write("b.wav", &[3.0; 8]);
        let mut sources = vec![
            AlignedSource::new(WavSampleReader::open(&a).unwrap(), 0, 8),
            AlignedSource::new(WavSampleReader::open(&b).unwrap(), 3, 8),
        ];
        let out = dir.join("merged.wav");
        let mut writer = wav_writer(&out, MERGE_SAMPLE_RATE).unwrap();
        let mut sink = |block: &[f32]| {
            block
                .iter()
                .try_for_each(|&s| writer.write_sample(s).map_err(|e| e.to_string()))
        };
        let written = merge_engine::merge_stream(
            MergeStrategy::MixAll,
            &mut sources,
            MERGE_SAMPLE_RATE as usize,
            &mut sink,
            &mut |_, _| Ok(()),
        )
        .unwrap();
        writer.finalize().unwrap();
        assert_eq!(written, 11);
        let merged = read_wav_samples(&out, 100).unwrap();
        assert_eq!(merged, vec![1.0, 1.0, 1.0, 2.0, 2.0, 2.0, 2.0, 2.0, 3.0, 3.0, 3.0]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_short_decoded_wav_is_an_error_not_silence() {
        let dir = test_dir("engine_short");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("short.wav");
        let mut writer = wav_writer(&path, MERGE_SAMPLE_RATE).unwrap();
        writer.write_sample(0.5f32).unwrap();
        writer.finalize().unwrap();
        let mut reader = WavSampleReader::open(&path).unwrap();
        let mut out = [0.0f32; 2];
        assert!(reader.read_exact_into(&mut out).is_err());
        let _ = std::fs::remove_dir_all(dir);
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

    #[test]
    fn the_cancel_flag_is_shared_and_stops_source_preparation() {
        let cancel = MergeCancel::default();
        let held_by_command = cancel.clone();
        assert!(!cancel.is_cancelled());
        assert!(cancel.check().is_ok());
        assert!(!(cancel.predicate())());

        // A Direct Audio SGMCA needs a temporary concat script over its
        // sessions; with the flag set before the merge reaches it, no script
        // is written and the merge stops.
        let dir = test_dir("cancel");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("session.sgmca");
        let bytes = crate::sgmca::fixtures::synthetic_direct(5, 312, 0, |_| 30);
        std::fs::write(&source, &bytes).unwrap();

        let options = crate::helpers::FeedOptions::default();
        let prepared = tauri::async_runtime::block_on(prepare_merge_feed(source.clone(), options, &cancel)).unwrap();
        let script = prepared.path().to_path_buf();
        assert_ne!(script, source, "uncancelled: the script is written");
        assert!(script.starts_with(crate::workdir::tests::test_root()));
        drop(prepared);
        assert!(!script.exists(), "the script goes with its guard");

        held_by_command.cancel();
        assert!(cancel.is_cancelled());
        assert!((cancel.predicate())());
        let error = tauri::async_runtime::block_on(prepare_merge_feed(source.clone(), options, &cancel)).unwrap_err();
        assert_eq!(error, MERGE_CANCELLED_MESSAGE);
        assert_eq!(std::fs::read(&source).unwrap(), bytes, "the source is only read");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_cancel_while_reading_the_sessions_leaves_no_temporary_file() {
        let cancel = MergeCancel::default();
        let dir = test_dir("cancel_copy");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("large.sgmca");
        let bytes = crate::sgmca::fixtures::synthetic_direct(20_000, 312, 0, |_| 30);
        std::fs::write(&source, &bytes).unwrap();
        // The page walk runs on a blocking worker; set the flag from its
        // predicate after a few checks so the cancel lands mid-walk.
        let checks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = checks.clone();
        let flag = cancel.clone();
        let predicate = Arc::new(move || {
            if counter.fetch_add(1, Ordering::Relaxed) >= 6 {
                flag.cancel();
            }
            flag.is_cancelled()
        });
        let feed = crate::helpers::prepare_audio_feed_with_cancel(
            &source,
            crate::helpers::FeedOptions::default(),
            Some(predicate.as_ref()),
        );
        assert!(feed.is_err(), "the preparation was cancelled");
        assert!(cancel.is_cancelled());
        assert!(
            crate::workdir::tests::thread_run_files().is_empty(),
            "no temporary file is left"
        );
        assert_eq!(std::fs::read(&source).unwrap(), bytes, "the source is only read");
        let _ = std::fs::remove_dir_all(dir);
    }
}
