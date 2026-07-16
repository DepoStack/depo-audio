use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use ort::session::Session;
use tauri::{AppHandle, Manager};

/// Result of the startup dlopen pre-flight (see lib.rs::setup_onnx_runtime).
/// ort 2.0.0-rc.12 hangs instead of erroring when its dylib fails to load,
/// so sessions must never be built unless the preflight succeeded.
static ORT_PREFLIGHT: OnceLock<Result<(), String>> = OnceLock::new();

pub(crate) fn set_ort_preflight(result: Result<(), String>) {
    let _ = ORT_PREFLIGHT.set(result);
}

// ── ONNX model loader ───────────────────────────────────────────────────────
//
// Lazily loads ONNX models on first use. The light models ship bundled in the
// app's resource directory under resources/models/.
//
// Heavier and optional models (the 38 MB speaker_embed.onnx, plus dnsmos and
// gtcrn) are not bundled — they are downloaded on demand from the models
// release into the app data directory, keeping the installer small.

/// Resolve a model file path. User-downloaded models live in the app data
/// directory (writable on installed apps — the resource dir is read-only in
/// Program Files and inside signed macOS bundles); bundled models live in
/// the resource directory. Data dir wins so downloads can update models.
pub(crate) fn model_path(app: &AppHandle, filename: &str) -> Result<PathBuf, String> {
    if let Ok(data_dir) = app.path().app_data_dir() {
        let downloaded = data_dir.join("models").join(filename);
        if downloaded.exists() {
            return Ok(downloaded);
        }
    }
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

/// Models that must run on the CPU execution provider. These have dynamic
/// input shapes and/or recurrent state, which CoreML (macOS) and DirectML
/// (Windows) handle pathologically: compilation can take minutes or wedge
/// outright — a per-file recompile of Silero VAD was enough to trip the
/// scan's 150-second stall watchdog on real machines ("Detecting speech"
/// froze, then the file was skipped). Both models are tiny; CPU inference is
/// milliseconds per call and, more importantly, predictable.
fn cpu_only(filename: &str) -> bool {
    matches!(filename, "silero_vad.onnx" | "speaker_seg_int8.onnx")
}

/// A loaded model session shared across scan passes. EP compilation and hash
/// verification are paid once per app run instead of once per pass per file.
pub(crate) type SharedSession = Arc<Mutex<Session>>;

static SESSION_CACHE: OnceLock<Mutex<HashMap<PathBuf, SharedSession>>> = OnceLock::new();

/// Cached variant of load_session for the scan passes: the first call per
/// model loads (and on macOS/Windows EP-compiles) the session; every later
/// pass and file reuses it. Callers lock the inner mutex for inference,
/// which also serializes concurrent use of one model across passes.
pub(crate) fn cached_session(path: &PathBuf) -> Result<SharedSession, String> {
    let cache = SESSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(s) = cache.lock().map_err(|_| "Model session cache poisoned".to_string())?.get(path) {
        return Ok(s.clone());
    }
    // Load outside the cache lock: EP compilation can take seconds and must
    // not block another pass's cache hit on a different model.
    let shared: SharedSession = Arc::new(Mutex::new(load_session(path)?));
    let mut guard = cache.lock().map_err(|_| "Model session cache poisoned".to_string())?;
    Ok(guard.entry(path.clone()).or_insert_with(|| shared.clone()).clone())
}

/// Load an ONNX session with hardware acceleration and optional integrity check.
/// Returns Err if ONNX Runtime is not installed — the app continues without AI features.
pub(crate) fn load_session(path: &PathBuf) -> Result<Session, String> {
    match ORT_PREFLIGHT.get() {
        Some(Ok(())) => {}
        Some(Err(e)) => return Err(format!("AI features unavailable: {}", e)),
        // Preflight never ran (tests, tooling): allow an explicit env override
        None if std::env::var("ORT_DYLIB_PATH").is_ok() => {}
        None => return Err("AI features unavailable: ONNX Runtime was not initialized".into()),
    }

    let name = crate::safety::safe_display(path);

    // Verify model integrity if a hash is known
    if let Some(expected_hash) = known_model_hash(&name) {
        verify_model_hash(path, expected_hash)?;
    }

    // Catch panics from missing ONNX Runtime (load-dynamic mode)
    let _accelerate = !cpu_only(&name);
    let result = std::panic::catch_unwind(|| {
        Session::builder().and_then(|mut b| {
            #[cfg(target_os = "macos")]
            if _accelerate {
                b = match b.with_execution_providers([
                    ort::execution_providers::CoreMLExecutionProvider::default().build(),
                ]) {
                    Ok(builder) => builder,
                    Err(_) => Session::builder()?,
                };
            }
            #[cfg(target_os = "windows")]
            if _accelerate {
                b = match b.with_execution_providers([
                    ort::execution_providers::DirectMLExecutionProvider::default().build(),
                ]) {
                    Ok(builder) => builder,
                    Err(_) => Session::builder()?,
                };
            }
            b.commit_from_file(path)
        })
    });

    match result {
        Ok(Ok(session)) => Ok(session),
        Ok(Err(e)) => Err(format!("Failed to load model {}: {}", name, e)),
        Err(_) => Err("ONNX Runtime not available. AI features are disabled. Install onnxruntime to enable them.".into()),
    }
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
        // Upstream sig_bak_ovr.onnx from microsoft/DNS-Challenge (the file
        // once committed here was an HTML error page — this is the real one)
        "dnsmos_sig_bak_ovr.onnx"  => Some("269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd"),
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

    // sha2 0.11's finalize() returns an `Array` that doesn't implement
    // LowerHex, so hex-encode the digest bytes explicitly.
    let digest = hasher.finalize();
    let hash: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
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
        "silero_vad.onnx",
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

// ── Model catalog ───────────────────────────────────────────────────────────

/// Metadata for each downloadable model.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ModelInfo {
    pub filename: String,
    pub display_name: String,
    pub description: String,
    pub size_mb: f64,
    pub feature: String,
    pub required: bool,
    pub installed: bool,
    pub recommended: bool,
    pub download_url: String,
}

/// Full model catalog with install status and recommendations.
pub(crate) fn model_catalog(app: &AppHandle) -> Vec<ModelInfo> {
    let caps = detect_capabilities(app);

    const BASE_URL: &str = "https://github.com/mayes/depo-audio/releases/download/models-v1";

    // size_mb values reflect the actual model files on disk
    let catalog = vec![
        ("silero_vad.onnx", "Silero VAD", "Voice activity detection — identifies speech vs silence", 2.1, "Speech Detection", true,
         format!("{}/silero_vad.onnx", BASE_URL)),
        ("smart-turn-v3-int8.onnx", "Smart Turn v3", "Detects speaker turns in court recordings", 8.2, "Turn Detection", false,
         format!("{}/smart-turn-v3-int8.onnx", BASE_URL)),
        ("dfn3_enc.onnx", "DeepFilterNet3 Encoder", "High-quality noise removal encoder", 1.9, "Noise Removal (Best)", false,
         format!("{}/dfn3_enc.onnx", BASE_URL)),
        ("dfn3_erb_dec.onnx", "DeepFilterNet3 ERB Decoder", "High-quality noise removal ERB decoder", 3.1, "Noise Removal (Best)", false,
         format!("{}/dfn3_erb_dec.onnx", BASE_URL)),
        ("dfn3_df_dec.onnx", "DeepFilterNet3 DF Decoder", "High-quality noise removal DF decoder", 3.2, "Noise Removal (Best)", false,
         format!("{}/dfn3_df_dec.onnx", BASE_URL)),
        ("flashsr.onnx", "FlashSR", "Neural bandwidth extension for phone/narrow-band audio", 0.5, "Clarity Enhancement", false,
         format!("{}/flashsr.onnx", BASE_URL)),
        ("dnsmos_sig_bak_ovr.onnx", "DNSMOS", "Audio quality scoring (1-5 scale)", 1.1, "Quality Scoring", false,
         format!("{}/dnsmos_sig_bak_ovr.onnx", BASE_URL)),
        ("speaker_seg_int8.onnx", "Speaker Segmentation", "Detects when different speakers are talking", 1.5, "Speaker Detection", false,
         format!("{}/speaker_seg_int8.onnx", BASE_URL)),
        ("speaker_embed.onnx", "Speaker Embedding", "Creates voice fingerprints for speaker identification", 37.8, "Speaker Detection", false,
         format!("{}/speaker_embed.onnx", BASE_URL)),
    ];

    catalog.into_iter().map(|(filename, name, desc, size, feature, required, url)| {
        let installed = model_path(app, filename).is_ok();
        let recommended = match feature {
            "Speech Detection" => true,
            "Noise Removal (Best)" => caps.tier == "high",
            "Turn Detection" => true,
            "Clarity Enhancement" => caps.tier != "low",
            "Quality Scoring" => true,
            "Speaker Detection" => caps.tier != "low",
            _ => false,
        };
        ModelInfo {
            filename: filename.to_string(),
            display_name: name.to_string(),
            description: desc.to_string(),
            size_mb: size,
            feature: feature.to_string(),
            required,
            installed,
            recommended,
            download_url: url.to_string(),
        }
    }).collect()
}

/// Download a model from its URL to the models directory.
pub(crate) async fn download_model(app: &AppHandle, filename: &str) -> Result<String, String> {
    let catalog = model_catalog(app);
    let info = catalog.iter()
        .find(|m| m.filename == filename)
        .ok_or_else(|| format!("Unknown model: {}", filename))?;

    // Download into the app data dir — the resource dir is not writable for
    // installed apps (Program Files on Windows, signed bundle on macOS)
    let data_dir = app.path().app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {}", e))?;
    let models_dir = data_dir.join("models");
    std::fs::create_dir_all(&models_dir)
        .map_err(|e| format!("Cannot create models dir: {}", e))?;

    let dest = models_dir.join(filename);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let resp = client.get(&info.download_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }

    let bytes = resp.bytes().await
        .map_err(|e| format!("Download read error: {}", e))?;

    // Guard against saving an error page as a "model" (it has happened):
    // ONNX files are protobuf, never text
    let looks_textual = bytes.starts_with(b"<") || bytes.starts_with(b"{")
        || bytes.starts_with(b"Not Found");
    if bytes.len() < 10_000 || looks_textual {
        return Err("Download did not return a valid model file".into());
    }

    // Write to temp file first, then rename (atomic)
    let tmp = dest.with_extension("tmp");
    std::fs::write(&tmp, &bytes)
        .map_err(|e| format!("Write error: {}", e))?;

    // Verify hash if known
    if let Some(expected) = known_model_hash(filename) {
        if let Err(e) = verify_model_hash(&tmp, expected) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Downloaded model failed integrity check: {}", e));
        }
    }

    std::fs::rename(&tmp, &dest)
        .map_err(|e| format!("Cannot move model into place: {}", e))?;

    Ok(format!("Downloaded {} ({:.1} MB)", info.display_name, info.size_mb))
}

/// Delete a downloaded model.
pub(crate) fn delete_model(app: &AppHandle, filename: &str) -> Result<(), String> {
    // Don't allow deleting required models
    let catalog = model_catalog(app);
    if let Some(info) = catalog.iter().find(|m| m.filename == filename) {
        if info.required {
            return Err("Cannot delete required model".into());
        }
    }

    let path = model_path(app, filename)?;
    std::fs::remove_file(&path)
        .map_err(|e| format!("Cannot delete model: {}", e))
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
    /// Whether the optional DCCRN+ de-reverb model is installed.
    /// It is not bundled or downloadable — see scripts/export_dccrn.py.
    pub dereverb_available: bool,
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

    let dereverb_available = model_path(app, "dccrn_plus.onnx").is_ok();

    SystemCapabilities {
        cpu_cores,
        ram_mb,
        apple_silicon,
        accelerator: accelerator.into(),
        accelerator_desc: accelerator_desc.into(),
        recommended_denoise: recommended_denoise.into(),
        recommend_speaker_detection,
        recommend_enhance,
        dereverb_available,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end check that the `ort` crate can load a real ONNX Runtime
    /// dylib and run the bundled Silero VAD model with the exact tensors
    /// vad.rs builds. Ignored by default: requires ORT_DYLIB_PATH pointing
    /// at a real libonnxruntime. Run with `cargo test -- --ignored`.
    ///
    /// Known caveat: in some sandboxed Linux containers ort's environment
    /// initialization deadlocks (upstream rc.12 bug: any init failure
    /// re-enters its API OnceLock) even when the same runtime works via
    /// Python's bindings. Run this on a desktop OS.
    #[test]
    #[ignore]
    fn ort_loads_and_runs_silero_vad() {
        let dylib = std::env::var("ORT_DYLIB_PATH")
            .expect("set ORT_DYLIB_PATH to a real libonnxruntime to run this test");
        // Pre-flight the dylib ourselves: ort 2.0.0-rc.12 deadlocks instead
        // of erroring when its dylib fails to load (see setup_onnx_runtime)
        match unsafe { libloading::Library::new(&dylib) } {
            Ok(lib) => {
                std::mem::forget(lib);
                set_ort_preflight(Ok(()));
            }
            Err(e) => {
                eprintln!("skipping: dylib does not load in this environment: {e}");
                return;
            }
        }
        let model = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources/models/silero_vad.onnx");
        let mut session = load_session(&model).expect("session should load");

        let chunk = ndarray::Array2::<f32>::zeros((1, 512));
        let state = ndarray::Array3::<f32>::zeros((2, 1, 128));
        let sr = ndarray::Array1::from_vec(vec![16000i64]);

        let outputs = session
            .run(ort::inputs![
                "input" => ort::value::Tensor::from_array(chunk).unwrap(),
                "state" => ort::value::Tensor::from_array(state).unwrap(),
                "sr" => ort::value::Tensor::from_array(sr).unwrap()
            ])
            .expect("inference should run");

        let prob = outputs
            .get("output")
            .and_then(|v| v.try_extract_tensor::<f32>().ok())
            .and_then(|t| t.1.first().copied())
            .expect("output tensor");
        assert!(prob.is_finite() && (0.0..=1.0).contains(&prob), "prob = {}", prob);
    }
}
