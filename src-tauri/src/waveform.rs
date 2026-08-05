use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Serialize;
use tauri::AppHandle;
use tokio::sync::oneshot;

use crate::ffmpeg::{ensure_ftr_decoder, probe_duration_cancellable, sidecar_output_cancellable};
use crate::helpers::{detect_format_for_path, ffmpeg_bin_name, input_codec_args, prepare_audio_feed_cancellable};

const MAX_WAVEFORM_SECONDS: f64 = 10.0 * 60.0;
const WAVEFORM_SAMPLE_RATE: u32 = 1_000;
const WAVEFORM_BUCKETS: usize = 2_048;
const WAVEFORM_CACHE_ENTRIES: usize = 32;
const CANCELLED_REQUEST_TOMBSTONES: usize = 256;
const MAX_REQUEST_ID_BYTES: usize = 128;
const WAVEFORM_CANCELLED_MESSAGE: &str = "Waveform request cancelled";

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct WaveformPeak {
    min: f32,
    max: f32,
}

type SharedPeaks = Arc<Vec<WaveformPeak>>;
type WaveformResult = Result<SharedPeaks, String>;
type Waiter = oneshot::Sender<WaveformResult>;

/// A cache identity tied to the exact file version that was inspected. The
/// canonical path deduplicates aliases while length and modified time prevent
/// a stale envelope from surviving an ordinary in-place edit.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FileKey {
    canonical_path: PathBuf,
    modified: SystemTime,
    len: u64,
}

impl FileKey {
    fn inspect(path: &Path) -> Result<Self, String> {
        crate::safety::check_file_safe(path)?;
        let canonical_path =
            std::fs::canonicalize(path).map_err(|error| format!("Cannot resolve the selected recording: {error}"))?;
        crate::safety::check_file_safe(&canonical_path)?;
        let metadata = std::fs::metadata(&canonical_path)
            .map_err(|error| format!("Cannot inspect the selected recording: {error}"))?;
        let modified = metadata
            .modified()
            .map_err(|error| format!("Cannot read the recording's modified time: {error}"))?;
        Ok(Self {
            canonical_path,
            modified,
            len: metadata.len(),
        })
    }
}

struct Flight {
    token: u64,
    cancelled: Arc<AtomicBool>,
    waiters: HashMap<String, Waiter>,
}

#[derive(Clone)]
struct RequestLocation {
    key: FileKey,
    token: u64,
}

struct FlightStart {
    token: u64,
    cancelled: Arc<AtomicBool>,
}

enum Registration {
    Cached(SharedPeaks),
    Cancelled,
    Duplicate,
    Waiting {
        receiver: oneshot::Receiver<WaveformResult>,
        start: Option<FlightStart>,
    },
}

struct CancelAction {
    waiter: Option<Waiter>,
    cancel_flight: Option<Arc<AtomicBool>>,
    found: bool,
}

#[derive(Default)]
struct WaveformRegistry {
    cache: HashMap<FileKey, SharedPeaks>,
    lru: VecDeque<FileKey>,
    flights: HashMap<FileKey, Flight>,
    requests: HashMap<String, RequestLocation>,
    cancelled_requests: HashSet<String>,
    cancelled_order: VecDeque<String>,
    next_flight_token: u64,
}

impl WaveformRegistry {
    fn cached(&mut self, key: &FileKey) -> Option<SharedPeaks> {
        let peaks = self.cache.get(key)?.clone();
        self.touch_lru(key);
        Some(peaks)
    }

    fn touch_lru(&mut self, key: &FileKey) {
        if let Some(index) = self.lru.iter().position(|candidate| candidate == key) {
            self.lru.remove(index);
        }
        self.lru.push_back(key.clone());
    }

    fn cache_success(&mut self, key: FileKey, peaks: SharedPeaks) {
        self.cache.insert(key.clone(), peaks);
        self.touch_lru(&key);
        while self.cache.len() > WAVEFORM_CACHE_ENTRIES {
            if let Some(oldest) = self.lru.pop_front() {
                self.cache.remove(&oldest);
            } else {
                break;
            }
        }
    }

    fn remember_cancelled(&mut self, request_id: String) {
        if self.cancelled_requests.insert(request_id.clone()) {
            self.cancelled_order.push_back(request_id);
        }
        while self.cancelled_order.len() > CANCELLED_REQUEST_TOMBSTONES {
            if let Some(oldest) = self.cancelled_order.pop_front() {
                self.cancelled_requests.remove(&oldest);
            }
        }
    }

    fn take_cancelled(&mut self, request_id: &str) -> bool {
        if !self.cancelled_requests.remove(request_id) {
            return false;
        }
        if let Some(index) = self
            .cancelled_order
            .iter()
            .position(|candidate| candidate == request_id)
        {
            self.cancelled_order.remove(index);
        }
        true
    }

    fn register(&mut self, key: FileKey, request_id: String) -> Registration {
        if self.take_cancelled(&request_id) {
            return Registration::Cancelled;
        }
        if self.requests.contains_key(&request_id) {
            return Registration::Duplicate;
        }
        if let Some(peaks) = self.cached(&key) {
            return Registration::Cached(peaks);
        }

        let (sender, receiver) = oneshot::channel();
        if let Some(flight) = self.flights.get_mut(&key) {
            let token = flight.token;
            flight.waiters.insert(request_id.clone(), sender);
            self.requests.insert(request_id, RequestLocation { key, token });
            return Registration::Waiting { receiver, start: None };
        }

        self.next_flight_token = self.next_flight_token.wrapping_add(1).max(1);
        let token = self.next_flight_token;
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut waiters = HashMap::new();
        waiters.insert(request_id.clone(), sender);
        self.flights.insert(
            key.clone(),
            Flight {
                token,
                cancelled: cancelled.clone(),
                waiters,
            },
        );
        self.requests.insert(request_id, RequestLocation { key, token });

        Registration::Waiting {
            receiver,
            start: Some(FlightStart { token, cancelled }),
        }
    }

    fn detach(&mut self, request_id: &str) -> CancelAction {
        let Some(location) = self.requests.remove(request_id) else {
            self.remember_cancelled(request_id.to_string());
            return CancelAction {
                waiter: None,
                cancel_flight: None,
                found: false,
            };
        };

        let mut waiter = None;
        let mut cancel_flight = None;
        let mut remove_flight = false;
        if let Some(flight) = self.flights.get_mut(&location.key) {
            if flight.token == location.token {
                waiter = flight.waiters.remove(request_id);
                if flight.waiters.is_empty() {
                    cancel_flight = Some(flight.cancelled.clone());
                    remove_flight = true;
                }
            }
        }
        if remove_flight {
            self.flights.remove(&location.key);
        }

        CancelAction {
            found: waiter.is_some(),
            waiter,
            cancel_flight,
        }
    }

    fn complete(&mut self, key: &FileKey, token: u64, mut result: WaveformResult) -> (Vec<Waiter>, WaveformResult) {
        let matches = self.flights.get(key).is_some_and(|flight| flight.token == token);
        if !matches {
            return (Vec::new(), Err(WAVEFORM_CANCELLED_MESSAGE.into()));
        }

        let flight = self.flights.remove(key).expect("matching waveform flight exists");
        if flight.cancelled.load(Ordering::Acquire) {
            result = Err(WAVEFORM_CANCELLED_MESSAGE.into());
        }
        for request_id in flight.waiters.keys() {
            if self
                .requests
                .get(request_id)
                .is_some_and(|location| location.key == *key && location.token == token)
            {
                self.requests.remove(request_id);
            }
        }
        if let Ok(peaks) = &result {
            self.cache_success(key.clone(), peaks.clone());
        }
        (flight.waiters.into_values().collect(), result)
    }
}

/// Shared waveform lifecycle state. Cloning this value only clones the Arc;
/// it lets the detached decode task finish a flight without borrowing Tauri's
/// command-scoped State guard.
#[derive(Clone, Default)]
pub struct WaveformState {
    inner: Arc<Mutex<WaveformRegistry>>,
}

impl WaveformState {
    fn register(&self, key: FileKey, request_id: String) -> Registration {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .register(key, request_id)
    }

    fn finish(&self, key: &FileKey, token: u64, result: WaveformResult) {
        let (waiters, result) = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .complete(key, token, result);
        // Sending can wake command futures immediately, so never do it while
        // the registry mutex is held.
        for waiter in waiters {
            let _ = waiter.send(result.clone());
        }
    }

    pub(crate) fn cancel(&self, request_id: &str) -> Result<bool, String> {
        validate_request_id(request_id)?;
        let action = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .detach(request_id);
        // The existing sidecar helper observes this flag, kills the child, and
        // drains its event stream. Only the last departing waiter sets it.
        if let Some(cancelled) = action.cancel_flight {
            cancelled.store(true, Ordering::Release);
        }
        if let Some(waiter) = action.waiter {
            let _ = waiter.send(Err(WAVEFORM_CANCELLED_MESSAGE.into()));
        }
        Ok(action.found)
    }
}

fn validate_request_id(request_id: &str) -> Result<(), String> {
    if request_id.is_empty() || request_id.len() > MAX_REQUEST_ID_BYTES {
        return Err("Invalid waveform request id".into());
    }
    Ok(())
}

fn samples_to_peaks(bytes: &[u8], bucket_limit: usize) -> Result<Vec<WaveformPeak>, String> {
    if bytes.is_empty() || !bytes.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err("Waveform decoder returned invalid PCM data".into());
    }

    let samples = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    let bucket_count = bucket_limit.min(samples.len()).max(1);
    let samples_per_bucket = samples.len().div_ceil(bucket_count);
    let mut peaks = Vec::with_capacity(bucket_count);

    for chunk in samples.chunks(samples_per_bucket) {
        let mut min = 1.0_f32;
        let mut max = -1.0_f32;
        for sample in chunk {
            let sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            min = min.min(sample);
            max = max.max(sample);
        }
        peaks.push(WaveformPeak { min, max });
    }

    Ok(peaks)
}

async fn decode_waveform(app: &AppHandle, key: &FileKey, cancelled: Arc<AtomicBool>) -> WaveformResult {
    let is_cancelled = || cancelled.load(Ordering::Acquire);
    if is_cancelled() {
        return Err(WAVEFORM_CANCELLED_MESSAGE.into());
    }

    let path = key.canonical_path.to_string_lossy().to_string();
    let format = detect_format_for_path(&path).ok_or_else(|| "Unsupported audio format".to_string())?;
    if format.handler == "rejected" {
        return Err(format
            .note
            .unwrap_or_else(|| "This audio format cannot be decoded".into()));
    }
    if format.handler == "ftr" {
        ensure_ftr_decoder(app).await?;
        if is_cancelled() {
            return Err(WAVEFORM_CANCELLED_MESSAGE.into());
        }
    }

    let source = key.canonical_path.clone();
    let preparation_flag = cancelled.clone();
    let preparation_cancelled: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || preparation_flag.load(Ordering::Acquire));
    let (feed, _feed_guard) = prepare_audio_feed_cancellable(source, preparation_cancelled)
        .await
        .map_err(|error| {
            if is_cancelled() {
                WAVEFORM_CANCELLED_MESSAGE.into()
            } else {
                error
            }
        })?;
    if is_cancelled() {
        return Err(WAVEFORM_CANCELLED_MESSAGE.into());
    }

    let duration = probe_duration_cancellable(app, &feed, Some(&is_cancelled))
        .await
        .filter(|duration| duration.is_finite() && *duration > 0.0);
    if is_cancelled() {
        return Err(WAVEFORM_CANCELLED_MESSAGE.into());
    }
    let duration = duration.ok_or_else(|| "Could not determine the recording duration".to_string())?;
    if duration > MAX_WAVEFORM_SECONDS {
        return Err("Waveform disabled for recordings over 10 minutes. Playback is still available.".into());
    }

    let mut args = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
    ];
    args.extend(input_codec_args(&feed));
    args.extend([
        "-i".into(),
        feed.to_string_lossy().to_string(),
        "-map".into(),
        "0:a:0".into(),
        "-vn".into(),
        "-sn".into(),
        "-dn".into(),
        "-ac".into(),
        "1".into(),
        "-ar".into(),
        WAVEFORM_SAMPLE_RATE.to_string(),
        "-t".into(),
        MAX_WAVEFORM_SECONDS.to_string(),
        "-f".into(),
        "f32le".into(),
        "pipe:1".into(),
    ]);

    let output = sidecar_output_cancellable(app, ffmpeg_bin_name(), args, 120, Some(&is_cancelled)).await;
    if is_cancelled() {
        return Err(WAVEFORM_CANCELLED_MESSAGE.into());
    }
    let output = output.ok_or_else(|| "Waveform decoder timed out or could not start".to_string())?;
    if !output.success {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("unknown FFmpeg error");
        return Err(format!("Waveform decoder failed: {detail}"));
    }

    // The key is checked again only after FFmpeg has finished. A changed
    // source is neither delivered nor cached, even when FFmpeg exited cleanly.
    let restat_path = key.canonical_path.clone();
    let current_key = tauri::async_runtime::spawn_blocking(move || FileKey::inspect(&restat_path))
        .await
        .map_err(|error| format!("Could not re-inspect the recording: {error}"))??;
    if current_key != *key {
        return Err("The recording changed while its waveform was being decoded. Try again.".into());
    }
    if is_cancelled() {
        return Err(WAVEFORM_CANCELLED_MESSAGE.into());
    }

    samples_to_peaks(&output.stdout, WAVEFORM_BUCKETS).map(Arc::new)
}

/// Join or start a bounded waveform request. Disk inspection happens before
/// taking the registry lock; decode/probe work and waiter sends happen after it
/// is released.
pub(crate) async fn request_waveform(
    app: &AppHandle,
    state: &WaveformState,
    path: String,
    request_id: String,
) -> Result<Vec<WaveformPeak>, String> {
    validate_request_id(&request_id)?;
    let requested_path = PathBuf::from(path);
    let key = tauri::async_runtime::spawn_blocking(move || FileKey::inspect(&requested_path))
        .await
        .map_err(|error| format!("Could not inspect the recording: {error}"))??;

    match state.register(key.clone(), request_id) {
        Registration::Cached(peaks) => Ok(peaks.as_ref().clone()),
        Registration::Cancelled => Err(WAVEFORM_CANCELLED_MESSAGE.into()),
        Registration::Duplicate => Err("Duplicate waveform request id".into()),
        Registration::Waiting { receiver, start } => {
            if let Some(start) = start {
                let app = app.clone();
                let worker_state = state.clone();
                let worker_key = key.clone();
                tauri::async_runtime::spawn(async move {
                    let result = decode_waveform(&app, &worker_key, start.cancelled).await;
                    worker_state.finish(&worker_key, start.token, result);
                });
            }
            receiver
                .await
                .map_err(|_| WAVEFORM_CANCELLED_MESSAGE.to_string())?
                .map(|peaks| peaks.as_ref().clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn pcm(samples: &[f32]) -> Vec<u8> {
        samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()
    }

    fn key(index: u64) -> FileKey {
        FileKey {
            canonical_path: PathBuf::from(format!("recording-{index}.wav")),
            modified: UNIX_EPOCH + Duration::from_secs(index),
            len: index + 1,
        }
    }

    fn shared_peak(index: f32) -> SharedPeaks {
        Arc::new(vec![WaveformPeak {
            min: -index,
            max: index,
        }])
    }

    #[test]
    fn aggregates_pcm_into_bounded_min_max_pairs() {
        let peaks = samples_to_peaks(&pcm(&[-1.0, 0.5, -0.25, 0.75]), 2).unwrap();
        assert_eq!(
            peaks,
            vec![
                WaveformPeak { min: -1.0, max: 0.5 },
                WaveformPeak { min: -0.25, max: 0.75 },
            ]
        );
    }

    #[test]
    fn clamps_non_finite_and_out_of_range_samples() {
        let peaks = samples_to_peaks(&pcm(&[f32::NAN, -2.0, 2.0]), 3).unwrap();
        assert_eq!(
            peaks,
            vec![
                WaveformPeak { min: 0.0, max: 0.0 },
                WaveformPeak { min: -1.0, max: -1.0 },
                WaveformPeak { min: 1.0, max: 1.0 },
            ]
        );
    }

    #[test]
    fn rejects_empty_or_partial_float_streams() {
        assert!(samples_to_peaks(&[], 2).is_err());
        assert!(samples_to_peaks(&[0, 1, 2], 2).is_err());
    }

    #[test]
    fn successful_cache_is_a_32_entry_lru() {
        let mut registry = WaveformRegistry::default();
        for index in 0..WAVEFORM_CACHE_ENTRIES as u64 {
            registry.cache_success(key(index), shared_peak(index as f32));
        }
        assert!(registry.cached(&key(0)).is_some()); // promote the oldest entry
        registry.cache_success(key(32), shared_peak(32.0));

        assert_eq!(registry.cache.len(), WAVEFORM_CACHE_ENTRIES);
        assert!(registry.cached(&key(0)).is_some());
        assert!(registry.cached(&key(1)).is_none());
    }

    #[test]
    fn concurrent_callers_share_a_flight_but_cancel_independently() {
        let state = WaveformState::default();
        let first = state.register(key(1), "request-one".into());
        let second = state.register(key(1), "request-two".into());
        let (mut first_rx, flight_cancelled) = match first {
            Registration::Waiting {
                receiver,
                start: Some(start),
            } => (receiver, start.cancelled),
            _ => panic!("first caller should start the flight"),
        };
        let mut second_rx = match second {
            Registration::Waiting { receiver, start: None } => receiver,
            _ => panic!("second caller should join the flight"),
        };

        assert!(state.cancel("request-one").unwrap());
        assert!(!flight_cancelled.load(Ordering::Acquire));
        assert!(first_rx.try_recv().unwrap().is_err());
        assert!(matches!(second_rx.try_recv(), Err(oneshot::error::TryRecvError::Empty)));

        assert!(state.cancel("request-two").unwrap());
        assert!(flight_cancelled.load(Ordering::Acquire));
        assert!(second_rx.try_recv().unwrap().is_err());
    }

    #[test]
    fn cancel_before_registration_is_consumed_by_that_request() {
        let state = WaveformState::default();
        assert!(!state.cancel("request-early").unwrap());
        assert!(matches!(
            state.register(key(2), "request-early".into()),
            Registration::Cancelled
        ));
    }

    #[test]
    fn failed_flights_are_not_cached() {
        let state = WaveformState::default();
        let registration = state.register(key(3), "request-error".into());
        let (mut receiver, token) = match registration {
            Registration::Waiting {
                receiver,
                start: Some(start),
            } => (receiver, start.token),
            _ => panic!("request should start a flight"),
        };

        state.finish(&key(3), token, Err("decode failed".into()));
        assert!(receiver.try_recv().unwrap().is_err());
        let mut registry = state.inner.lock().unwrap();
        assert!(registry.cached(&key(3)).is_none());
    }
}
