use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

// ── In-memory audio buffer ───────────────────────────────────────────────────

/// In-memory PCM audio buffer for passing between processing stages.
/// Eliminates temp WAV files in the pipeline.
pub struct AudioBuffer {
    pub samples: Vec<f32>,   // interleaved samples
    pub channels: u16,
    pub sample_rate: u32,
}

impl AudioBuffer {
    /// Read a WAV file into an AudioBuffer
    pub fn from_wav(path: &Path) -> Result<Self, String> {
        let reader = hound::WavReader::open(path)
            .map_err(|e| format!("WAV read error: {}", e))?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => {
                // hound yields unshifted integers (a 16-bit sample stays in
                // ±32768), so scale by 2^(bits-1) — i32::MAX would attenuate
                // 16-bit PCM by ~96 dB into near-silence. Guard the shift:
                // extensible WAVs can declare arbitrary wValidBitsPerSample.
                if !(1..=32).contains(&spec.bits_per_sample) {
                    return Err(format!("Unsupported WAV bit depth: {}", spec.bits_per_sample));
                }
                let scale = (1i64 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .map(|s| s.unwrap_or(0) as f32 / scale)
                    .collect()
            }
            hound::SampleFormat::Float => reader
                .into_samples::<f32>()
                .map(|s| s.unwrap_or(0.0))
                .collect(),
        };
        Ok(Self { samples, channels: spec.channels, sample_rate: spec.sample_rate })
    }

    /// Write AudioBuffer to a WAV file
    pub fn to_wav(&self, path: &Path) -> Result<(), String> {
        let spec = hound::WavSpec {
            channels: self.channels,
            sample_rate: self.sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec)
            .map_err(|e| format!("WAV write error: {}", e))?;
        for &s in &self.samples {
            writer.write_sample(s).map_err(|e| format!("Write error: {}", e))?;
        }
        writer.finalize().map_err(|e| format!("Finalize error: {}", e))?;
        Ok(())
    }

    /// De-interleave into per-channel buffers
    pub fn channels_split(&self) -> Vec<Vec<f32>> {
        let ch = self.channels as usize;
        let frames = self.samples.len() / ch;
        (0..ch).map(|c| (0..frames).map(|f| self.samples[f * ch + c]).collect()).collect()
    }

    /// Re-interleave from per-channel buffers
    pub fn from_channels(channel_bufs: &[Vec<f32>], sample_rate: u32) -> Self {
        let ch = channel_bufs.len();
        // Use the longest channel so a model returning slightly different
        // per-channel lengths can't silently truncate a channel (shorter
        // channels are zero-padded to match).
        let frames = channel_bufs.iter().map(|b| b.len()).max().unwrap_or(0);
        let mut samples = Vec::with_capacity(frames * ch);
        for f in 0..frames {
            for buf in channel_bufs {
                samples.push(buf.get(f).copied().unwrap_or(0.0));
            }
        }
        Self { samples, channels: ch as u16, sample_rate }
    }
}

// ── Conversion types ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FormatInfo {
    pub key: String,
    pub name: String,
    pub vendor: String,
    pub status: String,
    pub handler: String,
    pub channels: Option<String>,
    pub note: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ConvertJob {
    pub id: String,
    pub src_path: String,
    pub out_dir: String,
    pub mode: String,
    pub format: String,
    pub rate: String,
    /// MP3 encoding bitrate in kbps (128, 192, or 320). Ignored for non-MP3 output.
    #[serde(default = "default_mp3_bitrate")]
    pub mp3_bitrate: u32,
    pub labels: Vec<String>,
    pub chan_vols: Vec<f64>,
    pub normalize: bool,
    pub trim: bool,
    pub fade: bool,
    pub fade_dur: f64,
    pub hpf: bool,
    pub case_name: Option<String>,
    // AI processing options
    #[serde(default)]
    pub denoise: bool,
    /// "fast" (RNNoise) or "best" (DeepFilterNet3)
    #[serde(default = "default_denoise_quality")]
    pub denoise_quality: String,
    #[serde(default)]
    pub auto_level: bool,
    #[serde(default)]
    pub declip: bool,
    #[serde(default)]
    pub enhance: bool,
    #[serde(default)]
    pub dereverb: bool,
    // Advanced processing settings (from Settings panel)
    #[serde(default = "default_hpf_cutoff")]
    pub hpf_cutoff: f64,
    #[serde(default = "default_normalize_lufs")]
    pub normalize_lufs: f64,
    #[serde(default = "default_normalize_tp")]
    pub normalize_tp: f64,
    #[serde(default = "default_silence_thresh")]
    pub silence_thresh: f64,
    #[serde(default = "default_ffmpeg_timeout")]
    pub ffmpeg_timeout: u32,
    #[serde(default = "default_max_file_size_gb")]
    pub max_file_size_gb: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct OutputFile {
    pub name: String,
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ConvertResult {
    pub files: Vec<OutputFile>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub id: String,
    pub seconds: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// Source duration in seconds, when known — lets the UI render a real
    /// percentage instead of an indeterminate bar during encoding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
}

// ── AI analysis types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TurnSegment {
    pub start: f64,
    pub end: f64,
    pub channel: u32,
    pub confidence: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisResult {
    pub channels: u32,
    pub duration: f64,
    pub sample_rate: u32,
    pub per_channel_lufs: Vec<f64>,
    pub per_channel_peak: Vec<f64>,
    pub has_clipping: bool,
    pub needs_leveling: bool,
    pub needs_denoise: bool,
    pub is_narrowband: bool,
    pub turns: Vec<TurnSegment>,
    pub channel_gains: Vec<f64>,
    pub recommendations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality_score: Option<QualityScoreResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_count: Option<u32>,
    /// Ratio of speech to total duration (0.0 - 1.0), from VAD analysis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speech_ratio: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QualityScoreResult {
    /// Speech signal quality (1-5)
    pub sig: f32,
    /// Background noise quality (1-5, higher = cleaner)
    pub bak: f32,
    /// Overall quality (1-5)
    pub ovr: f32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DoneEvent {
    pub id: String,
    pub files: Vec<OutputFile>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ErrorEvent {
    pub id: String,
    pub message: String,
}

// ── Library types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LibFile {
    pub path: String,
    pub format: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Participant {
    pub label: String,
    pub files: Vec<LibFile>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub date: String,
    pub source_file: String,
    pub source_name: String,
    pub participants: Vec<Participant>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Case {
    pub id: String,
    pub name: String,
    pub created_at: String,
    pub archived: bool,
    pub sessions: Vec<Session>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct Library {
    pub version: u32,
    pub cases: Vec<Case>,
}

// ── Prefs ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Prefs {
    pub theme: String,
    pub mode: String,
    pub format: String,
    pub rate: String,
    #[serde(default = "default_mp3_bitrate")]
    pub mp3_bitrate: u32,
    pub out_dir: String,
    pub labels: Vec<String>,
    pub chan_vols: Vec<f64>,
    pub normalize: bool,
    pub trim: bool,
    pub fade: bool,
    pub fade_dur: f64,
    pub hpf: bool,
    // AI processing
    #[serde(default)]
    pub denoise: bool,
    #[serde(default = "default_denoise_quality")]
    pub denoise_quality: String,
    #[serde(default)]
    pub auto_level: bool,
    #[serde(default)]
    pub declip: bool,
    #[serde(default)]
    pub enhance: bool,
    #[serde(default)]
    pub dereverb: bool,
    // Advanced settings
    #[serde(default = "default_hpf_cutoff")]
    pub hpf_cutoff: f64,
    #[serde(default = "default_normalize_lufs")]
    pub normalize_lufs: f64,
    #[serde(default = "default_normalize_tp")]
    pub normalize_tp: f64,
    #[serde(default = "default_silence_thresh")]
    pub silence_thresh: f64,
    #[serde(default = "default_fade_dur_setting")]
    pub default_fade_dur: f64,
    #[serde(default = "default_ffmpeg_timeout")]
    pub ffmpeg_timeout: u32,
    #[serde(default = "default_max_scan_depth")]
    pub max_scan_depth: u32,
    #[serde(default = "default_max_file_size_gb")]
    pub max_file_size_gb: f64,
    /// Startup output format; empty string means "remember last used".
    #[serde(default)]
    pub default_output_format: String,
    /// Startup output mode; empty string means "remember last used".
    #[serde(default)]
    pub default_output_mode: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            mode: "stereo".into(),
            format: "wav".into(),
            rate: "48000".into(),
            mp3_bitrate: 192,
            out_dir: "".into(),
            labels: vec!["Speaker 1".into(), "Speaker 2".into(), "Speaker 3".into(), "Speaker 4".into()],
            chan_vols: vec![1.0, 1.0, 1.0, 1.0],
            normalize: false,
            trim: false,
            fade: false,
            fade_dur: 0.5,
            hpf: false,
            denoise: false,
            denoise_quality: "fast".into(),
            auto_level: false,
            declip: false,
            enhance: false,
            dereverb: false,
            hpf_cutoff: 80.0,
            normalize_lufs: -16.0,
            normalize_tp: -1.5,
            silence_thresh: -50.0,
            default_fade_dur: 0.5,
            ffmpeg_timeout: 300,
            max_scan_depth: 5,
            max_file_size_gb: 2.0,
            default_output_format: "".into(),
            default_output_mode: "".into(),
        }
    }
}

fn default_denoise_quality() -> String { "fast".into() }
/// Default MP3 bitrate (kbps). Matches the historical fixed 192 kbps output.
fn default_mp3_bitrate() -> u32 { 192 }
fn default_hpf_cutoff() -> f64 { 80.0 }
fn default_normalize_lufs() -> f64 { -16.0 }
fn default_normalize_tp() -> f64 { -1.5 }
fn default_silence_thresh() -> f64 { -50.0 }
fn default_fade_dur_setting() -> f64 { 0.5 }
fn default_ffmpeg_timeout() -> u32 { 300 }
fn default_max_scan_depth() -> u32 { 5 }
fn default_max_file_size_gb() -> f64 { 2.0 }

// ── App state ─────────────────────────────────────────────────────────────────

pub struct AppState {
    pub library: Mutex<Library>,
    pub prefs: Mutex<Prefs>,
    pub lib_path: Mutex<Option<PathBuf>>,
    pub prefs_path: Mutex<Option<PathBuf>>,
    /// Scan cancellation epoch. A scan snapshots the value when it starts and
    /// aborts once it no longer matches; `cancel_scan_cmd` bumps it. Arc so
    /// blocking inference tasks can keep checking after the command future
    /// itself is gone.
    pub scan_epoch: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            library: Mutex::new(Library::default()),
            prefs: Mutex::new(Prefs::default()),
            lib_path: Mutex::new(None),
            prefs_path: Mutex::new(None),
            scan_epoch: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Wire-contract characterization: the frontend sends camelCase JSON and
    // relies on these defaults for fields older frontends omit. Breaking
    // either breaks every installed client at IPC time.

    #[test]
    fn convert_job_fills_defaults_for_omitted_fields() {
        let j: ConvertJob = serde_json::from_value(serde_json::json!({
            "id": "t", "srcPath": "/in.wav", "outDir": "", "mode": "stereo",
            "format": "wav", "rate": "48000", "labels": [], "chanVols": [],
            "normalize": false, "trim": false, "fade": false, "fadeDur": 0.5,
            "hpf": false, "caseName": null,
        })).expect("minimal job deserializes");
        assert_eq!(j.mp3_bitrate, 192);
        assert_eq!(j.denoise_quality, "fast");
        assert!(!j.denoise && !j.auto_level && !j.declip && !j.enhance && !j.dereverb);
        assert_eq!(j.hpf_cutoff, 80.0);
        assert_eq!(j.normalize_lufs, -16.0);
        assert_eq!(j.normalize_tp, -1.5);
        assert_eq!(j.silence_thresh, -50.0);
        assert_eq!(j.ffmpeg_timeout, 300);
        assert_eq!(j.max_file_size_gb, 2.0);
    }

    #[test]
    fn prefs_defaults_are_the_documented_out_of_box_experience() {
        let p = Prefs::default();
        assert_eq!(p.theme, "system");
        assert_eq!(p.mode, "stereo");
        assert_eq!(p.format, "wav");
        assert_eq!(p.rate, "48000");
        assert_eq!(p.mp3_bitrate, 192);
        assert_eq!(p.labels, vec!["Speaker 1", "Speaker 2", "Speaker 3", "Speaker 4"]);
        assert_eq!(p.chan_vols, vec![1.0; 4]);
        // Empty string = "remember last used" sentinel for startup format/mode
        assert_eq!(p.default_output_format, "");
        assert_eq!(p.default_output_mode, "");
    }

    #[test]
    fn prefs_serialize_as_camel_case() {
        let v = serde_json::to_value(Prefs::default()).unwrap();
        // Spot-check the rename policy on multi-word fields
        assert!(v.get("mp3Bitrate").is_some());
        assert!(v.get("outDir").is_some());
        assert!(v.get("defaultOutputFormat").is_some());
        assert!(v.get("mp3_bitrate").is_none());
    }

    #[test]
    fn prefs_from_older_versions_still_load() {
        // A prefs.json written before the AI/advanced fields existed must
        // still deserialize (all newer fields have serde defaults)
        let p: Prefs = serde_json::from_value(serde_json::json!({
            "theme": "dark", "mode": "split", "format": "mp3", "rate": "44100",
            "outDir": "/out", "labels": ["A"], "chanVols": [1.0],
            "normalize": true, "trim": false, "fade": false, "fadeDur": 0.5, "hpf": false,
        })).expect("v0.6-era prefs deserialize");
        assert_eq!(p.theme, "dark");
        assert_eq!(p.mp3_bitrate, 192);
        assert_eq!(p.max_scan_depth, 5);
    }

    #[test]
    fn library_json_shape_is_stable() {
        // The on-disk library.json shape: snake-free camelCase for Session/Case,
        // plain fields for LibFile/Participant
        let lib: Library = serde_json::from_value(serde_json::json!({
            "version": 1,
            "cases": [{
                "id": "c1", "name": "Smith", "createdAt": "2025-01-01T00:00:00Z",
                "archived": false,
                "sessions": [{
                    "id": "s1", "date": "2025-01-01",
                    "sourceFile": "/a.wav", "sourceName": "a.wav",
                    "participants": [{ "label": "Original",
                        "files": [{ "path": "/a.mp3", "format": "mp3", "size": 5 }] }]
                }]
            }]
        })).expect("library.json deserializes");
        assert_eq!(lib.cases[0].sessions[0].participants[0].files[0].size, 5);
        let back = serde_json::to_value(&lib).unwrap();
        assert!(back["cases"][0].get("createdAt").is_some());
        assert!(back["cases"][0]["sessions"][0].get("sourceFile").is_some());
    }
}
