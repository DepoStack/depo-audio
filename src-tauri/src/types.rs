use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

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
pub struct ConvertJob {
    pub id: String,
    pub src_path: String,
    pub out_dir: String,
    pub mode: String,
    pub format: String,
    pub rate: String,
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
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            theme: "system".into(),
            mode: "stereo".into(),
            format: "wav".into(),
            rate: "48000".into(),
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
        }
    }
}

fn default_denoise_quality() -> String { "fast".into() }

// ── App state ─────────────────────────────────────────────────────────────────

pub struct AppState {
    pub library: Mutex<Library>,
    pub prefs: Mutex<Prefs>,
    pub lib_path: Mutex<Option<PathBuf>>,
    pub prefs_path: Mutex<Option<PathBuf>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            library: Mutex::new(Library::default()),
            prefs: Mutex::new(Prefs::default()),
            lib_path: Mutex::new(None),
            prefs_path: Mutex::new(None),
        }
    }
}
