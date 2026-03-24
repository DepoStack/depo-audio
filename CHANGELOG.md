# Changelog

## [0.6.0] - 2026-03-24

### Added
- **Smart Audio Cleanup** — new AI Enhance panel that scans audio and auto-detects issues (noise, level imbalance, clipping, narrow bandwidth)
- **Remove Background Noise** — neural noise suppression via nnnoiseless (RNNoise, fast) or DeepFilterNet3 ONNX (best quality)
- **Balance Speaker Volume** — turn-aware auto-leveling using Pipecat Smart Turn v3 ONNX model + ebur128 loudness analysis
- **Fix Clipped Audio** — FFmpeg adeclip filter, auto-enabled when clipping detected
- **Improve Audio Clarity** — FlashSR ONNX neural bandwidth extension for phone recordings (16kHz to 48kHz)
- **Quality Scoring** — DNSMOS model rates audio quality on a 1-5 scale before and after processing
- **Speaker Count Detection** — pyannote speaker segmentation detects how many speakers are present
- **Hardware-Aware Recommendations** — detects CPU cores, RAM, and Apple Silicon to recommend optimal processing settings
- **Global Audio Player** (new tab) — play any audio file without conversion, with color-coded speaker tracks and editable labels
- **Court Software Detection** — scans for installed Case CATalyst, FTR Gold, Eclipse, DigitalCAT, CourtSmart and lists available jobs for import
- **Denoise Quality Selector** — choose between "Fast" (RNNoise) and "Best quality" (DeepFilterNet3) per conversion
- **Phase-Aware Progress** — shows "Listening to audio...", "Cleaning up audio...", "Converting..." during AI processing stages

### Changed
- App tagline updated to "Audio Converter & Enhancer" to reflect broader use
- Default channel labels changed to "Speaker 1-4" (editable, no longer court-specific)
- Drop zone now lists standard formats first (WAV, MP3, FLAC, Opus) before court formats
- Three-tab layout: Convert | Player | Library
- Processing chain preview includes AI steps (Denoise, Enhance, De-clip, Auto-Level)
- Channel Mix sliders auto-disabled and show "auto" when Balance Speaker Volume is on

### Architecture
- New two-step conversion pipeline: Rust AI processing (denoise, enhance) writes temp WAV, then FFmpeg handles DSP + format conversion
- ONNX Runtime integration via `ort` crate for running AI models natively in Rust
- `nnnoiseless` crate for pure-Rust RNNoise noise suppression
- Bundled ONNX models (~57MB): Smart Turn v3, FlashSR, DeepFilterNet3, DNSMOS, speaker segmentation + embedding
- Dynamic channel handling throughout (mono, stereo, multi-channel)

## [0.5.0] - 2026-03-21

### Added
- **AAC/M4A output format** — 128 kbps AAC in M4A container for Apple device compatibility
- **Configurable MP3 bitrate** — choose 128, 192, 256, or 320 kbps from the format selector
- **.dcr format detection** — Liberty Court Recorder files are identified with guidance to export via Liberty software
- **Dynamic channel count** — auto-detects 4–16 channel recordings and resizes the UI accordingly
- **Retry button** — failed conversions show a retry action instead of requiring re-import
- **Extended channel palette** — 16 distinct colors for multi-channel recordings (up from 4)

### Changed
- Drop zone moved above configuration panels for a more natural top-down workflow
- Progress bar now shows real percentage derived from FFmpeg duration probe
- Stereo mix uses actual detected channel count instead of hardcoded 4

### Fixed
- Vite 8 build — switched minifier from deprecated esbuild to oxc

## [0.4.0] - 2026-03-12

### Added
- **Track preview** — audition individual channels before conversion for quick identification (Reporter, Witness, Attorney 1 & 2)
- **Mix audition** — preview the final stereo mix with per-channel volume and processing applied
- **Processing preview** — hear the effect of high-pass filter, normalization, and silence trimming before committing
- **Cancel conversion** — stop an in-progress conversion at any time; partially written files are cleaned up automatically
- **Atomic file writes** — output files are written to a temporary path first, then moved into place, preventing corrupted files on crash or cancel
- **Global audio player** — persistent playback bar with queue support across Convert and Library views
- **React error boundary** — catches unhandled UI crashes and offers a recovery button instead of a blank screen
- Dependabot configuration for automated dependency updates
- MIT LICENSE file and CODEOWNERS

### Changed
- Upgraded React 18 → 19 and react-dom to 19.2.4
- Upgraded Vite 7 → 8 (Rolldown-based bundler)
- Upgraded @vitejs/plugin-react 4 → 6 (Babel removed, Oxc-based transforms)
- Updated Cargo dependencies including tauri-plugin-updater to 2.10.0
- Updated CI actions (actions/checkout v6, actions/setup-node v6)
- Improved conversion progress UI with smoother status updates

### Security
- Bumped `tar` crate 0.4.44 → 0.4.45 (fixes symlink-directory collision chmod attack)
- Bumped `rustls-webpki` 0.103.9 → 0.103.10

## [0.3.0] - 2026-03-12

### Added
- In-app auto-updater powered by GitHub Releases — update notifications with change notes and skip option
- Shared audio player hook (`useAudioPlayer`) reducing code duplication
- ARIA labels and semantic roles throughout the UI for accessibility
- Update banner component with download progress and release notes viewer

### Fixed
- **Critical:** `ConvertJob` serde deserialization — added `camelCase` rename so frontend `srcPath`/`outDir`/`chanVols`/`fadeDur` fields map correctly to Rust struct
- macOS Intel CI builds — replaced deprecated `macos-13` runner with cross-compilation on `macos-latest`
- Toggle component accessibility — changed from `<div>` to `<button role="switch">` with keyboard support

### Changed
- ConvertTab now accepts grouped prop objects (`prefs`, `fileDrop`, `conversion`) instead of 30+ individual props
- Consolidated duplicate `.proc-chain` CSS definitions
- Updated README with full feature docs, architecture overview, and format support table
- Replaced placeholder SECURITY.md with actual project information

## [0.2.0] - 2026-03-11

### Changed
- Refactored Rust backend from single 806-line `lib.rs` into 7 focused modules:
  - `types.rs` — all struct/enum definitions
  - `helpers.rs` — format registry, path helpers, SGMCA stripping
  - `ffmpeg.rs` — FFmpeg probe, filter chain, sidecar runner
  - `conversion.rs` — core conversion logic (stereo/keep/split)
  - `persistence.rs` — library and preferences storage
  - `commands.rs` — Tauri command handlers
  - `lib.rs` — module declarations and app builder
- Added `protocol-asset` feature to Tauri dependency
- Skipped macOS code signing for unsigned builds

## [0.1.0] - Initial Release

### Added
- SGMCA, BWF, FTR/TRM, DigitalCAT .dm, and standard audio format support
- Three output modes: Mix to Stereo, Keep Original, Split Channels
- Per-channel volume controls (Reporter, Witness, Attorney 1 & 2)
- Processing chain: High-pass filter, loudness normalization, silence trim, fade in/out
- Output formats: WAV, MP3 (192 kbps), FLAC, Opus (64 kbps VBR)
- Case/witness auto-filing library with inline playback and re-export
- Batch processing support
- Windows (x64) and macOS (Apple Silicon + Intel) builds
