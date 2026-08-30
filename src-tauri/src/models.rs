use std::path::{Component, Path};

use tauri::{AppHandle, Manager};

/// Learned-model files that older builds could bundle, download, or load.
///
/// v1.0.3 deliberately exposes no learned-model execution or download path.
/// Existing regular files in writable app data are surfaced only so users can
/// remove legacy storage. This exact allowlist is not an availability registry.
struct LegacyModelFile {
    filename: &'static str,
    display_name: &'static str,
}

const LEGACY_MODEL_FILES: &[LegacyModelFile] = &[
    LegacyModelFile {
        filename: "dfn3_config.ini",
        display_name: "DeepFilterNet3 configuration (legacy)",
    },
    LegacyModelFile {
        filename: "dfn3_enc.onnx",
        display_name: "DeepFilterNet3 encoder (legacy)",
    },
    LegacyModelFile {
        filename: "dfn3_erb_dec.onnx",
        display_name: "DeepFilterNet3 ERB decoder (legacy)",
    },
    LegacyModelFile {
        filename: "dfn3_df_dec.onnx",
        display_name: "DeepFilterNet3 DF decoder (legacy)",
    },
    LegacyModelFile {
        filename: "speaker_embed.onnx",
        display_name: "Speaker embedding (legacy)",
    },
    LegacyModelFile {
        filename: "silero_vad.onnx",
        display_name: "Silero VAD (legacy)",
    },
    LegacyModelFile {
        filename: "smart-turn-v3-int8.onnx",
        display_name: "Smart Turn v3 (legacy)",
    },
    LegacyModelFile {
        filename: "flashsr.onnx",
        display_name: "FlashSR (legacy)",
    },
    LegacyModelFile {
        filename: "dnsmos_sig_bak_ovr.onnx",
        display_name: "DNSMOS (legacy)",
    },
    LegacyModelFile {
        filename: "speaker_seg_int8.onnx",
        display_name: "Speaker segmentation (legacy)",
    },
    LegacyModelFile {
        filename: "dccrn_plus.onnx",
        display_name: "DCCRN+ developer preview (legacy)",
    },
];

fn legacy_model_file(filename: &str) -> Option<&'static LegacyModelFile> {
    LEGACY_MODEL_FILES.iter().find(|model| model.filename == filename)
}

/// Model names cross an IPC boundary for the deletion-only cleanup command.
/// Restrict them to one ordinary filename before joining them to app data.
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

/// One deletion-only legacy row. There is intentionally no download URL,
/// required state, recommendation, or executable capability attached to it.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LegacyModelInfo {
    pub filename: String,
    pub display_name: String,
    pub description: String,
    pub size_mb: f64,
    pub installed: bool,
    pub removable: bool,
}

fn legacy_model_catalog(models_dir: &Path) -> Vec<LegacyModelInfo> {
    LEGACY_MODEL_FILES
        .iter()
        .filter_map(|legacy| {
            let path = models_dir.join(legacy.filename);
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if !metadata.file_type().is_file() {
                return None;
            }
            Some(LegacyModelInfo {
                filename: legacy.filename.to_string(),
                display_name: legacy.display_name.to_string(),
                description: "Legacy learned-model file — not loaded by this release. Remove it to reclaim storage"
                    .to_string(),
                size_mb: (metadata.len() as f64 / (1024.0 * 1024.0) * 10.0).round() / 10.0,
                installed: true,
                removable: true,
            })
        })
        .collect()
}

/// Return only exact legacy app-data files that can be deleted. An empty list
/// is the expected state for a clean v1.0.3 installation.
pub(crate) fn legacy_model_cleanup_catalog(app: &AppHandle) -> Vec<LegacyModelInfo> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| legacy_model_catalog(&dir.join("models")))
        .unwrap_or_default()
}

/// Delete one exact regular legacy file from writable app data. Unknown names,
/// directories, symlinks, and every install/download operation fail closed.
pub(crate) fn delete_legacy_model(app: &AppHandle, filename: &str) -> Result<(), String> {
    validate_model_filename(filename)?;
    if legacy_model_file(filename).is_none() {
        return Err(format!("Unknown legacy model file: {filename}"));
    }

    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?;
    let path = data_dir.join("models").join(filename);
    let metadata = std::fs::symlink_metadata(&path).map_err(|_| "Legacy model file not found".to_string())?;
    if !metadata.file_type().is_file() {
        return Err("Legacy model cleanup accepts regular files only".into());
    }
    std::fs::remove_file(&path).map_err(|e| format!("Cannot delete legacy model file: {e}"))
}

/// Conservative local processing information. The learned-model booleans are
/// retained on the wire so older frontends/preferences migrate safely, but
/// they are always false for v1.0.3.
#[derive(Debug, serde::Serialize, serde::Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SystemCapabilities {
    pub cpu_cores: Option<usize>,
    pub ram_mb: Option<u64>,
    pub apple_silicon: bool,
    pub accelerator: String,
    pub accelerator_desc: String,
    pub recommended_denoise: String,
    pub recommend_speaker_detection: bool,
    pub recommend_enhance: bool,
    pub dereverb_available: bool,
    pub tier: String,
}

pub(crate) fn detect_capabilities() -> SystemCapabilities {
    let cpu_cores = std::thread::available_parallelism().ok().map(|p| p.get());
    let ram_mb = estimate_ram_mb();
    let apple_silicon = cfg!(target_arch = "aarch64") && cfg!(target_os = "macos");
    let tier = performance_tier(cpu_cores, ram_mb);

    SystemCapabilities {
        cpu_cores,
        ram_mb,
        apple_silicon,
        accelerator: "cpu".into(),
        accelerator_desc: "CPU".into(),
        recommended_denoise: "unavailable".into(),
        recommend_speaker_detection: false,
        recommend_enhance: false,
        dereverb_available: false,
        tier: tier.into(),
    }
}

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
    fn performance_tier_requires_measured_cpu_and_ram() {
        assert_eq!(performance_tier(Some(8), Some(8000)), "high");
        assert_eq!(performance_tier(Some(4), Some(4000)), "mid");
        assert_eq!(performance_tier(Some(16), None), "low");
        assert_eq!(performance_tier(None, Some(32768)), "low");
    }

    #[test]
    fn released_capabilities_never_expose_learned_processing() {
        let capabilities = detect_capabilities();
        assert_eq!(capabilities.accelerator, "cpu");
        assert_eq!(capabilities.recommended_denoise, "unavailable");
        assert!(!capabilities.recommend_speaker_detection);
        assert!(!capabilities.recommend_enhance);
        assert!(!capabilities.dereverb_available);
    }

    #[test]
    fn legacy_cleanup_allowlist_is_exact_and_download_free() {
        for filename in [
            "dfn3_config.ini",
            "dfn3_enc.onnx",
            "dfn3_erb_dec.onnx",
            "dfn3_df_dec.onnx",
            "speaker_embed.onnx",
            "silero_vad.onnx",
            "smart-turn-v3-int8.onnx",
            "flashsr.onnx",
            "dnsmos_sig_bak_ovr.onnx",
            "speaker_seg_int8.onnx",
            "dccrn_plus.onnx",
        ] {
            assert!(legacy_model_file(filename).is_some(), "missing {filename}");
        }
        assert!(legacy_model_file("anything.onnx").is_none());
    }

    #[test]
    fn existing_legacy_file_is_removal_only() {
        let models_dir =
            std::env::temp_dir().join(format!("depoaudio-legacy-model-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&models_dir).unwrap();
        std::fs::write(models_dir.join("flashsr.onnx"), vec![0_u8; 1024]).unwrap();

        let catalog = legacy_model_catalog(&models_dir);
        assert_eq!(catalog.len(), 1);
        let legacy = &catalog[0];
        assert_eq!(legacy.filename, "flashsr.onnx");
        assert!(legacy.installed && legacy.removable);

        std::fs::remove_dir_all(models_dir).unwrap();
    }

    #[test]
    fn cleanup_filename_rejects_path_escape_forms() {
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
            assert!(validate_model_filename(name).is_err(), "accepted {name:?}");
        }
    }
}
