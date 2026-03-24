use std::path::PathBuf;

use ort::session::Session;
use tauri::{AppHandle, Manager};

// ── ONNX model loader ───────────────────────────────────────────────────────
//
// Lazily loads ONNX models on first use. Models are bundled in the app's
// resource directory under resources/models/.
//
// Heavier models (speaker_embed.onnx) can optionally be downloaded on demand
// rather than bundled, to keep the installer small.

/// Resolve a model file path in the app's resource directory.
pub(crate) fn model_path(app: &AppHandle, filename: &str) -> Result<PathBuf, String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("Cannot resolve resource dir: {}", e))?;
    let path = resource_dir.join("resources").join("models").join(filename);
    if path.exists() {
        Ok(path)
    } else {
        Err(format!("Model not found: {}", filename))
    }
}

/// Load an ONNX session from a model file.
pub(crate) fn load_session(path: &PathBuf) -> Result<Session, String> {
    let name = crate::safety::safe_display(path);
    Session::builder()
        .and_then(|mut b| b.commit_from_file(path))
        .map_err(|e| format!("Failed to load model {}: {}", name, e))
}

// ── Model availability check ────────────────────────────────────────────────

/// Check which models are available on this installation.
/// Useful for UI to show/hide features based on bundled models.
pub(crate) fn available_models(app: &AppHandle) -> Vec<String> {
    let models = [
        "smart-turn-v3-int8.onnx",
        "flashsr.onnx",
        "dfn3_enc.onnx",
        "dnsmos_sig_bak_ovr.onnx",
        "speaker_seg_int8.onnx",
        "speaker_embed.onnx",
    ];
    models
        .iter()
        .filter(|m| model_path(app, m).is_ok())
        .map(|m| m.to_string())
        .collect()
}

// ── Hardware-aware recommendations ──────────────────────────────────────────

/// System capabilities for recommending which AI features to enable.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemCapabilities {
    /// Number of logical CPU cores.
    pub cpu_cores: usize,
    /// Available RAM in MB.
    pub ram_mb: u64,
    /// Whether the system is Apple Silicon (fast ONNX inference).
    pub apple_silicon: bool,
    /// Recommended denoise quality ("fast" or "best").
    pub recommended_denoise: String,
    /// Whether speaker detection is recommended (needs 38MB model + RAM).
    pub recommend_speaker_detection: bool,
    /// Whether bandwidth extension is recommended.
    pub recommend_enhance: bool,
    /// General performance tier: "low", "mid", "high".
    pub tier: String,
}

/// Detect system capabilities and recommend features.
pub(crate) fn detect_capabilities(app: &AppHandle) -> SystemCapabilities {
    let cpu_cores = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(2);

    // Estimate available RAM (platform-specific)
    let ram_mb = estimate_ram_mb();

    // Detect Apple Silicon
    let apple_silicon = cfg!(target_arch = "aarch64") && cfg!(target_os = "macos");

    // Performance tier
    let tier = if cpu_cores >= 8 && ram_mb >= 8000 {
        "high"
    } else if cpu_cores >= 4 && ram_mb >= 4000 {
        "mid"
    } else {
        "low"
    };

    // Recommendations based on tier
    let recommended_denoise = if tier == "high" || apple_silicon {
        "best" // DeepFilterNet3
    } else {
        "fast" // RNNoise
    };

    let recommend_speaker_detection = tier != "low"
        && available_models(app).contains(&"speaker_seg_int8.onnx".to_string());

    let recommend_enhance = available_models(app).contains(&"flashsr.onnx".to_string());

    SystemCapabilities {
        cpu_cores,
        ram_mb,
        apple_silicon,
        recommended_denoise: recommended_denoise.into(),
        recommend_speaker_detection,
        recommend_enhance,
        tier: tier.into(),
    }
}

#[cfg(target_os = "macos")]
fn estimate_ram_mb() -> u64 {
    use std::process::Command;
    Command::new("sysctl")
        .arg("-n")
        .arg("hw.memsize")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .map(|bytes| bytes / (1024 * 1024))
        .unwrap_or(4096)
}

#[cfg(target_os = "windows")]
fn estimate_ram_mb() -> u64 {
    // On Windows, use systeminfo or WMI — simplified fallback
    8192
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn estimate_ram_mb() -> u64 {
    // Linux: read /proc/meminfo
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|v| v.parse::<u64>().ok())
                .map(|kb| kb / 1024)
        })
        .unwrap_or(4096)
}
