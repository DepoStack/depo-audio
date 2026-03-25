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

/// Maximum time (ms) for a single model inference call.
const INFERENCE_TIMEOUT_MS: i32 = 30_000; // 30 seconds

/// Load an ONNX session with hardware acceleration and optional integrity check.
pub(crate) fn load_session(path: &PathBuf) -> Result<Session, String> {
    let name = crate::safety::safe_display(path);

    // Verify model integrity if a hash is known
    if let Some(expected_hash) = known_model_hash(&name) {
        verify_model_hash(path, expected_hash)?;
    }

    let result = Session::builder().and_then(|mut b| {
        // Try hardware acceleration. with_execution_providers consumes the builder,
        // so we only call it if we have a provider to add.
        #[cfg(target_os = "macos")]
        {
            b = match b.with_execution_providers([
                ort::execution_providers::CoreMLExecutionProvider::default().build(),
            ]) {
                Ok(builder) => builder,
                Err(_) => Session::builder()?, // Fallback: new CPU-only builder
            };
        }
        #[cfg(target_os = "windows")]
        {
            b = match b.with_execution_providers([
                ort::execution_providers::DirectMLExecutionProvider::default().build(),
            ]) {
                Ok(builder) => builder,
                Err(_) => Session::builder()?,
            };
        }
        b.commit_from_file(path)
    });

    result.map_err(|e| format!("Failed to load model {}: {}", name, e))
}

/// SHA256 hashes of known bundled models.
/// Add hashes here after downloading models to verify integrity.
fn known_model_hash(filename: &str) -> Option<&'static str> {
    match filename {
        "silero_vad.onnx"          => Some("a4a068cd6cf1ea8355b84327595838ca748ec29a25bc91fc82e6c299ccdc5808"),
        "smart-turn-v3-int8.onnx"  => Some("3d072c8fb04446955a365b533686e7e06015ad09929bb824b910c72ff89f5be1"),
        "flashsr.onnx"             => Some("e255c76b227f16f7f392cc43677c38bd2c5aa129f042a2ba3eb03fb29e470c7a"),
        "dfn3_enc.onnx"            => Some("7c5399d3da8a50ebef1c1a0ae421b33376aa5e45d0e92df16da7e83c9c131916"),
        "dfn3_erb_dec.onnx"        => Some("ab669a1d10afe20911728b33053a452071042317a90581092b325da7b2f9d895"),
        "dfn3_df_dec.onnx"         => Some("23114ce3b0f6464b763ee62f7bb8aab6b2a129a21eabd5bcfe59413db05f278a"),
        "dnsmos_sig_bak_ovr.onnx"  => Some("81c57ef0f69a2aa9a25dc878fba7534c9278de2769ecf5c221382d36929c5b5b"),
        "speaker_seg_int8.onnx"    => Some("d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d"),
        "speaker_embed.onnx"       => Some("1a331345f04805badbb495c775a6ddffcdd1a732567d5ec8b3d5749e3c7a5e4b"),
        _ => None,
    }
}

/// Verify a file's SHA256 hash matches the expected value.
fn verify_model_hash(path: &PathBuf, expected: &str) -> Result<(), String> {
    use sha2::{Sha256, Digest};
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Cannot open model for verification: {}", e))?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("Read error: {}", e))?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }

    let hash = format!("{:x}", hasher.finalize());
    if hash != expected {
        return Err(format!(
            "Model integrity check failed for {}. Expected hash prefix {}..., got {}...",
            crate::safety::safe_display(path),
            &expected[..12.min(expected.len())],
            &hash[..12],
        ));
    }

    Ok(())
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
    /// Whether the system is Apple Silicon (CoreML acceleration).
    pub apple_silicon: bool,
    /// Detected accelerator: "cpu", "coreml", "directml", "rocm", "xdna", "openvino".
    pub accelerator: String,
    /// Human-readable accelerator description.
    pub accelerator_desc: String,
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

    // Detect available hardware accelerator
    let (accelerator, accelerator_desc) = detect_accelerator(apple_silicon);

    // Performance tier — accelerators boost the tier
    let has_accel = accelerator != "cpu";
    let tier = if has_accel || (cpu_cores >= 8 && ram_mb >= 8000) {
        "high"
    } else if cpu_cores >= 4 && ram_mb >= 4000 {
        "mid"
    } else {
        "low"
    };

    // Recommendations based on tier
    let recommended_denoise = if tier == "high" {
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
        accelerator: accelerator.into(),
        accelerator_desc: accelerator_desc.into(),
        recommended_denoise: recommended_denoise.into(),
        recommend_speaker_detection,
        recommend_enhance,
        tier: tier.into(),
    }
}

/// Detect available hardware accelerator for AI inference.
/// Returns (id, human description).
fn detect_accelerator(apple_silicon: bool) -> (&'static str, &'static str) {
    // Apple Silicon: CoreML via ANE (Apple Neural Engine)
    if apple_silicon {
        return ("coreml", "Apple Neural Engine (CoreML)");
    }

    // Windows: check for DirectML (AMD/Intel/NVIDIA GPUs) and Intel OpenVINO
    #[cfg(target_os = "windows")]
    {
        // Check for AMD XDNA NPU (Ryzen AI)
        // The XDNA driver creates a device at a known path
        if std::path::Path::new("C:\\Windows\\System32\\DriverStore\\FileRepository").exists() {
            // Heuristic: check for AMD IPU driver
            if let Ok(entries) = std::fs::read_dir("C:\\Windows\\System32\\drivers") {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if name.contains("xdna") || name.contains("amdxdna") || name.contains("aie") {
                        return ("xdna", "AMD Ryzen AI (XDNA NPU)");
                    }
                }
            }

            // Check for Intel NPU (Meteor Lake+)
            if let Ok(entries) = std::fs::read_dir("C:\\Windows\\System32\\drivers") {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_lowercase();
                    if name.contains("intel_npu") || name.contains("intelnpu") {
                        return ("openvino", "Intel AI Boost (NPU)");
                    }
                }
            }

            // DirectML is available on all Windows 10+ with any GPU
            return ("directml", "DirectML (GPU acceleration)");
        }
    }

    // Linux: check for ROCm (AMD GPU)
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new("/opt/rocm").exists() {
            return ("rocm", "AMD ROCm (GPU acceleration)");
        }
        // Check for Intel OpenVINO
        if std::path::Path::new("/opt/intel/openvino").exists() {
            return ("openvino", "Intel OpenVINO");
        }
    }

    ("cpu", "CPU only")
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
