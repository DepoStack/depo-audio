use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
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
// Optional DNSMOS is not bundled; users can install it from the models
// release. The experimental DCCRN+ dereverb model is developer-provisioned
// only and is deliberately absent from the downloadable catalog.

struct ObsoleteModelDownload {
    filename: &'static str,
    display_name: &'static str,
}

/// Exact filenames formerly exposed by the model catalog but never used by a
/// released workflow. An existing app-data copy is shown only as removable
/// legacy storage; these files remain absent from availability and downloads.
const OBSOLETE_MODEL_DOWNLOADS: &[ObsoleteModelDownload] = &[
    ObsoleteModelDownload {
        filename: "dfn3_enc.onnx",
        display_name: "DeepFilterNet3 encoder (legacy)",
    },
    ObsoleteModelDownload {
        filename: "dfn3_erb_dec.onnx",
        display_name: "DeepFilterNet3 ERB decoder (legacy)",
    },
    ObsoleteModelDownload {
        filename: "dfn3_df_dec.onnx",
        display_name: "DeepFilterNet3 DF decoder (legacy)",
    },
    ObsoleteModelDownload {
        filename: "speaker_embed.onnx",
        display_name: "Speaker embedding (legacy)",
    },
];

fn obsolete_model_download(filename: &str) -> Option<&'static ObsoleteModelDownload> {
    OBSOLETE_MODEL_DOWNLOADS.iter().find(|model| model.filename == filename)
}

/// Resolve a model file path. User-downloaded models live in the app data
/// directory (writable on installed apps — the resource dir is read-only in
/// Program Files and inside signed macOS bundles); bundled models live in
/// the resource directory. Data dir wins so downloads can update models.
pub(crate) fn model_path(app: &AppHandle, filename: &str) -> Result<PathBuf, String> {
    validate_model_filename(filename)?;
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

/// Model names cross an IPC boundary for download/delete commands. Restrict
/// them to one ordinary filename so joining them to the model directory can
/// never escape via `..`, an absolute path, a Windows drive prefix, or ADS.
fn validate_model_filename(filename: &str) -> Result<(), String> {
    if filename.is_empty() || filename.contains(['/', '\\', ':']) || filename.chars().any(char::is_control) {
        return Err("Invalid model filename".into());
    }

    let mut components = Path::new(filename).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(name)), None) if name == std::ffi::OsStr::new(filename) => Ok(()),
        _ => Err("Invalid model filename".into()),
    }
}

/// Models that must run on the CPU execution provider. These have dynamic
/// input shapes and/or recurrent state, which CoreML can handle
/// pathologically: compilation can take minutes or wedge
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
/// model loads (and on macOS may EP-compile) the session; every later
/// pass and file reuses it. Callers lock the inner mutex for inference,
/// which also serializes concurrent use of one model across passes.
pub(crate) fn cached_session(path: &PathBuf) -> Result<SharedSession, String> {
    let cache = SESSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(s) = cache
        .lock()
        .map_err(|_| "Model session cache poisoned".to_string())?
        .get(path)
    {
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
                b = match b
                    .with_execution_providers([ort::execution_providers::CoreMLExecutionProvider::default().build()])
                {
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
        Err(_) => {
            Err("ONNX Runtime not available. AI features are disabled. Install onnxruntime to enable them.".into())
        }
    }
}

/// SHA256 hashes of known bundled models.
/// Add hashes here after downloading models to verify integrity.
fn known_model_hash(filename: &str) -> Option<&'static str> {
    match filename {
        "silero_vad.onnx" => Some("a4a068cd6cf1ea8355b84327595838ca748ec29a25bc91fc82e6c299ccdc5808"),
        "smart-turn-v3-int8.onnx" => Some("3d072c8fb04446955a365b533686e7e06015ad09929bb824b910c72ff89f5be1"),
        "flashsr.onnx" => Some("e255c76b227f16f7f392cc43677c38bd2c5aa129f042a2ba3eb03fb29e470c7a"),
        // Upstream sig_bak_ovr.onnx from microsoft/DNS-Challenge (the file
        // once committed here was an HTML error page — this is the real one)
        "dnsmos_sig_bak_ovr.onnx" => Some("269fbebdb513aa23cddfbb593542ecc540284a91849ac50516870e1ac78f6edd"),
        "speaker_seg_int8.onnx" => Some("d582f4b4c6b48205de7e0643c57df0df5615a3c176189be3fc461e9d18827b5d"),
        _ => None,
    }
}

/// Verify a file's SHA256 hash matches the expected value.
fn verify_model_hash(path: &PathBuf, expected: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;

    let mut file = std::fs::File::open(path).map_err(|e| format!("Cannot open model for verification: {}", e))?;

    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf).map_err(|e| format!("Read error: {}", e))?;
        if n == 0 {
            break;
        }
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

struct ModelSpec {
    filename: &'static str,
    display_name: &'static str,
    description: &'static str,
    size_mb: f64,
    feature: &'static str,
    required: bool,
}

const MODEL_DOWNLOAD_BASE_URL: &str = "https://github.com/DepoStack/depo-audio/releases/download/models-v1";

// This single reviewed list backs availability reporting, the Settings model
// catalog, and the download allowlist.
const RELEASED_MODEL_SPECS: &[ModelSpec] = &[
    ModelSpec {
        filename: "silero_vad.onnx",
        display_name: "Silero VAD",
        description: "Voice activity detection — identifies speech vs silence",
        size_mb: 2.1,
        feature: "Speech Detection",
        required: true,
    },
    ModelSpec {
        filename: "smart-turn-v3-int8.onnx",
        display_name: "Smart Turn v3",
        description: "Detects speaker turns in court recordings",
        size_mb: 8.2,
        feature: "Turn Detection",
        required: false,
    },
    ModelSpec {
        filename: "flashsr.onnx",
        display_name: "FlashSR",
        description: "Neural bandwidth extension for phone/narrow-band audio",
        size_mb: 0.5,
        feature: "Clarity Enhancement",
        required: false,
    },
    ModelSpec {
        filename: "dnsmos_sig_bak_ovr.onnx",
        display_name: "DNSMOS",
        description: "Audio quality scoring (1-5 scale)",
        size_mb: 1.1,
        feature: "Quality Scoring",
        required: false,
    },
    ModelSpec {
        filename: "speaker_seg_int8.onnx",
        display_name: "Speaker Segmentation",
        description: "Estimates active speaker slots; does not identify voices",
        size_mb: 1.5,
        feature: "Speaker Activity",
        required: false,
    },
];

fn released_model_spec(filename: &str) -> Option<&'static ModelSpec> {
    RELEASED_MODEL_SPECS.iter().find(|model| model.filename == filename)
}

// ── Model availability check ────────────────────────────────────────────────

/// Check which models are available on this installation.
/// Useful for UI to show/hide features based on bundled models.
pub(crate) fn available_models(app: &AppHandle) -> Vec<String> {
    RELEASED_MODEL_SPECS
        .iter()
        .filter(|model| model_path(app, model.filename).is_ok())
        .map(|model| model.filename.to_string())
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
    /// True only for a copy in the writable app-data model directory.
    /// Bundled resource models are installed but cannot be removed in place.
    pub removable: bool,
    pub recommended: bool,
    pub download_url: String,
}

fn legacy_model_catalog(models_dir: &Path) -> Vec<ModelInfo> {
    OBSOLETE_MODEL_DOWNLOADS
        .iter()
        .filter_map(|obsolete| {
            let path = models_dir.join(obsolete.filename);
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if !metadata.file_type().is_file() {
                return None;
            }
            Some(ModelInfo {
                filename: obsolete.filename.to_string(),
                display_name: obsolete.display_name.to_string(),
                description: "Legacy unused file — not used by this release. Remove it to reclaim storage".to_string(),
                size_mb: (metadata.len() as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0,
                feature: "Legacy unused file".to_string(),
                required: false,
                installed: true,
                removable: true,
                recommended: false,
                download_url: String::new(),
            })
        })
        .collect()
}

/// Full model catalog with install status and recommendations.
pub(crate) fn model_catalog(app: &AppHandle) -> Vec<ModelInfo> {
    let caps = detect_capabilities(app);

    let mut catalog: Vec<ModelInfo> = RELEASED_MODEL_SPECS
        .iter()
        .map(|model| {
            let installed = model_path(app, model.filename).is_ok();
            let removable = app
                .path()
                .app_data_dir()
                .ok()
                .map(|dir| dir.join("models").join(model.filename).is_file())
                .unwrap_or(false);
            let recommended = match model.feature {
                "Speech Detection" => true,
                "Turn Detection" => true,
                "Clarity Enhancement" => caps.tier != "low",
                "Quality Scoring" => true,
                "Speaker Activity" => caps.tier != "low",
                _ => false,
            };
            ModelInfo {
                filename: model.filename.to_string(),
                display_name: model.display_name.to_string(),
                description: model.description.to_string(),
                size_mb: model.size_mb,
                feature: model.feature.to_string(),
                required: model.required,
                installed,
                removable,
                recommended,
                download_url: format!("{}/{}", MODEL_DOWNLOAD_BASE_URL, model.filename),
            }
        })
        .collect();

    // Earlier builds allowed these unused files to be downloaded. Surface an
    // entry only when the exact regular file is already in writable app data,
    // giving the user a Remove action without exposing an install path.
    if let Ok(data_dir) = app.path().app_data_dir() {
        let models_dir = data_dir.join("models");
        catalog.extend(legacy_model_catalog(&models_dir));
    }

    catalog
}

/// Download a model from its URL to the models directory.
pub(crate) async fn download_model(app: &AppHandle, filename: &str) -> Result<String, String> {
    const MAX_MODEL_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;
    if obsolete_model_download(filename).is_some() {
        return Err("Legacy unused models cannot be downloaded".into());
    }
    let catalog = model_catalog(app);
    let info = catalog
        .iter()
        .find(|m| m.filename == filename)
        .ok_or_else(|| format!("Unknown model: {}", filename))?;

    // Download into the app data dir — the resource dir is not writable for
    // installed apps (Program Files on Windows, signed bundle on macOS)
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {}", e))?;
    let models_dir = data_dir.join("models");
    std::fs::create_dir_all(&models_dir).map_err(|e| format!("Cannot create models dir: {}", e))?;

    let dest = models_dir.join(filename);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("HTTP client error: {}", e))?;

    let mut resp = client
        .get(&info.download_url)
        .send()
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed: HTTP {}", resp.status()));
    }

    if resp
        .content_length()
        .is_some_and(|length| length > MAX_MODEL_DOWNLOAD_BYTES as u64)
    {
        return Err("Model download exceeds the 64 MB safety limit".into());
    }

    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("Download read error: {}", e))? {
        if bytes.len().saturating_add(chunk.len()) > MAX_MODEL_DOWNLOAD_BYTES {
            return Err("Model download exceeds the 64 MB safety limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }

    // Guard against saving an error page as a "model" (it has happened):
    // ONNX files are protobuf, never text
    let looks_textual = bytes.starts_with(b"<") || bytes.starts_with(b"{") || bytes.starts_with(b"Not Found");
    if bytes.len() < 10_000 || looks_textual {
        return Err("Download did not return a valid model file".into());
    }

    // Write to temp file first, then rename (atomic)
    let tmp = dest.with_extension(format!("tmp.{}", uuid::Uuid::new_v4().simple()));
    let write_result = (|| -> std::io::Result<()> {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(&bytes)?;
        file.sync_all()
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Write error: {error}"));
    }

    // Verify hash if known
    if let Some(expected) = known_model_hash(filename) {
        if let Err(e) = verify_model_hash(&tmp, expected) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("Downloaded model failed integrity check: {}", e));
        }
    }

    if let Err(error) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("Cannot move model into place: {error}"));
    }

    Ok(format!("Downloaded {} ({:.1} MB)", info.display_name, info.size_mb))
}

/// Delete a downloaded model.
pub(crate) fn delete_model(app: &AppHandle, filename: &str) -> Result<(), String> {
    validate_model_filename(filename)?;

    // Only exact optional catalog entries and the narrowly allowlisted legacy
    // downloads may be deleted. Unknown names must fail closed rather than
    // being passed to model_path, which also searches outside the writable
    // download directory for bundled resources.
    match released_model_spec(filename) {
        Some(model) if model.required => return Err("Cannot delete required model".into()),
        Some(_) => {}
        None if obsolete_model_download(filename).is_some() => {}
        None => return Err(format!("Unknown model: {}", filename)),
    }

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {}", e))?;
    let path = data_dir.join("models").join(filename);
    if !path.is_file() {
        return Err("Downloaded model not found".into());
    }
    std::fs::remove_file(&path).map_err(|e| format!("Cannot delete model: {}", e))
}

// ── Hardware-aware recommendations ──────────────────────────────────────────

/// System capabilities for recommending which AI features to enable.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemCapabilities {
    /// Number of logical CPU cores, when the operating system reports it.
    pub cpu_cores: Option<usize>,
    /// Total physical RAM in MB, when the operating system reports it.
    pub ram_mb: Option<u64>,
    /// Whether this build is running on Apple Silicon.
    pub apple_silicon: bool,
    /// Inference capability: "cpu" or "coreml-eligible".
    /// Eligibility does not prove that a particular model used CoreML.
    pub accelerator: String,
    /// Human-readable inference capability and fallback description.
    pub accelerator_desc: String,
    /// Recommended denoise quality. Only "fast" (RNNoise) is implemented.
    pub recommended_denoise: String,
    /// Whether active speaker-slot estimation is recommended for this tier.
    /// The bundled 1.5 MB segmentation model is used; voice embeddings are not.
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
    let cpu_cores = std::thread::available_parallelism().ok().map(|p| p.get());

    // Estimate available RAM (platform-specific)
    let ram_mb = estimate_ram_mb();

    // Detect Apple Silicon
    let apple_silicon = cfg!(target_arch = "aarch64") && cfg!(target_os = "macos");

    // Detect available hardware accelerator
    let (accelerator, accelerator_desc) = detect_accelerator(apple_silicon);

    // Performance tier is based only on measured CPU and memory capacity.
    let tier = performance_tier(cpu_cores, ram_mb);

    // DeepFilterNet3 is not a usable waveform pipeline yet. Recommend only
    // the implemented RNNoise path, regardless of hardware tier.
    let recommended_denoise = "fast";

    let recommend_speaker_detection =
        tier != "low" && available_models(app).contains(&"speaker_seg_int8.onnx".to_string());

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

/// Describe hardware-provider eligibility without asserting runtime use.
/// Returns (id, human description). ONNX Runtime can still fall back to CPU.
fn detect_accelerator(apple_silicon: bool) -> (&'static str, &'static str) {
    if apple_silicon {
        return ("coreml-eligible", "CoreML for eligible models; CPU fallback");
    }

    ("cpu", "CPU")
}

/// Classify measured hardware for conservative feature recommendations.
/// Missing either measurement cannot justify a higher tier.
fn performance_tier(cpu_cores: Option<usize>, ram_mb: Option<u64>) -> &'static str {
    match (cpu_cores, ram_mb) {
        (Some(cores), Some(ram)) if cores >= 8 && ram >= 8000 => "high",
        (Some(cores), Some(ram)) if cores >= 4 && ram >= 4000 => "mid",
        _ => "low",
    }
}

#[cfg(target_os = "macos")]
fn estimate_ram_mb() -> Option<u64> {
    use std::process::Command;
    Command::new("sysctl")
        .arg("-n")
        .arg("hw.memsize")
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .map(|bytes| bytes / (1024 * 1024))
}

#[cfg(target_os = "windows")]
fn estimate_ram_mb() -> Option<u64> {
    use std::mem::size_of;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    let succeeded = unsafe { GlobalMemoryStatusEx(&mut status) };
    (succeeded != 0).then_some(status.ullTotalPhys / (1024 * 1024))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn estimate_ram_mb() -> Option<u64> {
    // Linux: read /proc/meminfo
    std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
        s.lines()
            .find(|l| l.starts_with("MemTotal:"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
            .map(|kb| kb / 1024)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ort_binding_matches_bundled_runtime_api() {
        // release.yml and setup-dev.sh stage the backward-compatible 1.22.x
        // runtime. Keep the binding on API 21: API 22 enables ONNX Runtime's
        // experimental AutoEP path, which is not used by DepoAudio and fails
        // against Microsoft's macOS universal2 package.
        assert_eq!(ort::MINOR_VERSION, 21);
    }

    #[test]
    fn performance_tier_requires_measured_cpu_and_ram() {
        assert_eq!(performance_tier(Some(8), Some(8000)), "high");
        assert_eq!(performance_tier(Some(4), Some(4000)), "mid");
        assert_eq!(performance_tier(Some(16), None), "low");
        assert_eq!(performance_tier(None, Some(32768)), "low");
        assert_eq!(performance_tier(None, None), "low");
    }

    #[test]
    fn accelerator_description_states_eligibility_and_fallback() {
        assert_eq!(
            detect_accelerator(true),
            ("coreml-eligible", "CoreML for eligible models; CPU fallback")
        );
        assert_eq!(detect_accelerator(false), ("cpu", "CPU"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_reports_real_physical_memory() {
        assert!(estimate_ram_mb().is_some_and(|ram_mb| ram_mb > 0));
    }

    #[test]
    fn model_filename_accepts_one_plain_component() {
        assert!(validate_model_filename("speaker_embed.onnx").is_ok());
    }

    #[test]
    fn obsolete_download_cleanup_is_exact_and_not_a_catalog_wildcard() {
        for filename in [
            "dfn3_enc.onnx",
            "dfn3_erb_dec.onnx",
            "dfn3_df_dec.onnx",
            "speaker_embed.onnx",
        ] {
            assert!(obsolete_model_download(filename).is_some());
            assert!(RELEASED_MODEL_SPECS.iter().all(|model| model.filename != filename));
        }
        assert!(obsolete_model_download("silero_vad.onnx").is_none());
        assert!(obsolete_model_download("anything.onnx").is_none());
    }

    #[test]
    fn released_registry_is_the_exact_hash_pinned_five_model_set() {
        let filenames: Vec<_> = RELEASED_MODEL_SPECS.iter().map(|model| model.filename).collect();
        assert_eq!(
            filenames,
            vec![
                "silero_vad.onnx",
                "smart-turn-v3-int8.onnx",
                "flashsr.onnx",
                "dnsmos_sig_bak_ovr.onnx",
                "speaker_seg_int8.onnx",
            ]
        );
        for model in RELEASED_MODEL_SPECS {
            assert!(
                known_model_hash(model.filename).is_some(),
                "missing reviewed hash for {}",
                model.filename
            );
        }
        assert!(released_model_spec("silero_vad.onnx").is_some_and(|model| model.required));
        assert!(RELEASED_MODEL_SPECS
            .iter()
            .filter(|model| model.required)
            .all(|model| model.filename == "silero_vad.onnx"));
        assert!(released_model_spec("dfn3_enc.onnx").is_none());
        assert!(released_model_spec("speaker_embed.onnx").is_none());
    }

    #[test]
    fn existing_legacy_file_is_removal_only_and_disappears_after_deletion() {
        let models_dir =
            std::env::temp_dir().join(format!("depoaudio-legacy-model-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&models_dir).unwrap();
        let legacy_file = models_dir.join("speaker_embed.onnx");
        std::fs::write(&legacy_file, vec![0_u8; 1024]).unwrap();

        let catalog = legacy_model_catalog(&models_dir);
        assert_eq!(catalog.len(), 1);
        let legacy = &catalog[0];
        assert_eq!(legacy.filename, "speaker_embed.onnx");
        assert_eq!(legacy.feature, "Legacy unused file");
        assert!(legacy.installed);
        assert!(legacy.removable);
        assert!(!legacy.required);
        assert!(!legacy.recommended);
        assert!(legacy.download_url.is_empty());

        std::fs::remove_file(&legacy_file).unwrap();
        assert!(legacy_model_catalog(&models_dir).is_empty());
        std::fs::remove_dir_all(models_dir).unwrap();
    }

    #[test]
    fn model_filename_rejects_path_escape_forms() {
        for name in [
            "",
            ".",
            "..",
            "../library.json",
            "..\\library.json",
            "/tmp/model.onnx",
            "C:\\temp\\model.onnx",
            "model.onnx:stream",
            "models/model.onnx",
        ] {
            assert!(validate_model_filename(name).is_err(), "unexpectedly accepted {name:?}");
        }
    }

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
        let dylib =
            std::env::var("ORT_DYLIB_PATH").expect("set ORT_DYLIB_PATH to a real libonnxruntime to run this test");
        eprintln!("[ort-smoke] preflighting bundled runtime: {dylib}");
        // Pre-flight the dylib ourselves: ort 2.0.0-rc.12 deadlocks instead
        // of erroring when its dylib fails to load (see setup_onnx_runtime).
        // This is a release gate, so a staged library that cannot load must
        // fail rather than silently skip the smoke test.
        let lib = unsafe { libloading::Library::new(&dylib) }
            .unwrap_or_else(|e| panic!("ONNX Runtime dylib failed to load from {dylib}: {e}"));
        let version = crate::validate_onnx_runtime_api(&lib).expect("bundled runtime should expose the required API");
        assert!(
            version.starts_with("1.22."),
            "unexpected bundled runtime version: {version}"
        );
        eprintln!("[ort-smoke] runtime {version} exposes C API {}", ort::MINOR_VERSION);
        std::mem::forget(lib);
        set_ort_preflight(Ok(()));
        eprintln!("[ort-smoke] creating CPU-only Silero VAD session");
        let model = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/models/silero_vad.onnx");
        let mut session = load_session(&model).expect("session should load");
        eprintln!("[ort-smoke] session created; running inference");

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
        drop(outputs);
        drop(session);
        eprintln!("[ort-smoke] inference and session cleanup completed");
    }
}
