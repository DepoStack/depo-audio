# DepoAudio

**Desktop audio converter and enhancer for Windows and macOS.**

DepoAudio converts audio files between formats with smart cleanup that automatically detects and fixes common audio problems. Built for court reporters but works with any audio — mono, stereo, or multi-channel.

## Features

### Audio Conversion
- **Format Support** — Stenograph SGMCA, FTR/TRM, Broadcast WAV, DigitalCAT, plus standard formats (WAV, MP3, FLAC, M4A, OGG, Opus, WMA, and more)
- **Three Output Modes** — Mix to Stereo, Keep Original, or Split by Channel
- **Per-Channel Controls** — Label and volume-adjust each channel
- **Output Formats** — WAV (PCM 16-bit), MP3 (192 kbps), FLAC, Opus (64 kbps VBR), M4A
- **Batch Processing** — Queue and convert multiple files at once

### Smart Audio Cleanup
- **Automatic Detection** — Scan audio to detect noise, level imbalance, clipping, and narrow bandwidth
- **Remove Background Noise** — Neural noise suppression (RNNoise fast mode or DeepFilterNet3 best quality)
- **Balance Speaker Volume** — Turn-aware auto-leveling that measures loudness during active speech
- **Fix Clipped Audio** — Reconstructs distorted peaks from recordings that were too loud
- **Improve Audio Clarity** — Neural bandwidth extension for phone recordings and older audio (FlashSR)
- **Smart Turn Detection** — Identifies when each speaker starts and stops (Pipecat Smart Turn v3)
- **Quality Scoring** — Before/after quality rating on a 1-5 scale (DNSMOS)
- **Speaker Count Detection** — Automatically detects how many speakers are in the recording
- **Hardware-Aware** — Recommends processing options based on your machine's capabilities

### Global Audio Player
- **Play Any File** — Listen to audio files directly without conversion
- **Color-Coded Speakers** — Each track gets a distinct color for easy identification
- **Editable Labels** — Name each speaker in the playlist
- **Full Playback Controls** — Play, pause, skip, seek with per-track color seekbar

### Library & Software Detection
- **Case Library** — Auto-filed by case name with inline playback, search, archive, and re-export
- **Detect Court Software** — Scans for installed Case CATalyst, FTR Gold, Eclipse, DigitalCAT, CourtSmart
- **Import Jobs** — Browse detected software directories and import audio jobs directly

### General
- **Dark & Light Themes** — System-aware with manual override
- **100% Local** — All processing runs on your machine. No cloud, no uploads, no data leaves your computer

## Supported Formats

| Format | Vendor | Status |
|--------|--------|--------|
| SGMCA | Stenograph / Case CATalyst | Supported |
| FTR / TRM | For The Record | Experimental |
| BWF | CourtSmart / Various | Supported |
| DigitalCAT (.dm) | Stenovations | Experimental |
| WAV, MP3, FLAC, M4A, OGG, Opus, WMA, AIF | Standard | Supported |
| AES (Eclipse AudioSync) | Eclipse CAT | Unsupported (encrypted) |

## Installation

Download the latest preview release for your platform:

- **macOS** — `.dmg` (Apple Silicon and Intel)
- **Windows** — `.msi` installer or `.exe`

> [Latest Release](https://github.com/mayes/depo-audio/releases/latest)

**Note:** Preview builds are not code-signed yet. You'll need to bypass Gatekeeper on macOS or SmartScreen on Windows on first launch. See the [project README on GitHub](https://github.com/mayes/depo-audio#installation) for step-by-step instructions.

## Development Setup

### Prerequisites

- [Rust](https://rustup.rs/) (stable toolchain)
- [Node.js](https://nodejs.org/) 22+
- [Tauri CLI](https://v2.tauri.app/start/prerequisites/) (`cargo install tauri-cli`)

### FFmpeg Sidecars

Place FFmpeg and FFprobe binaries in `src-tauri/binaries/` with target-triple naming:

| Platform | Files |
|----------|-------|
| macOS ARM | `ffmpeg-aarch64-apple-darwin`, `ffprobe-aarch64-apple-darwin` |
| macOS Intel | `ffmpeg-x86_64-apple-darwin`, `ffprobe-x86_64-apple-darwin` |
| Windows x64 | `ffmpeg-x86_64-pc-windows-msvc.exe`, `ffprobe-x86_64-pc-windows-msvc.exe` |

Download from [ffmpeg.org](https://ffmpeg.org/download.html) or [evermeet.cx/ffmpeg](https://evermeet.cx/ffmpeg/) (macOS).

### Run

```bash
npm install
npm run tauri dev
```

### Build

```bash
npm run tauri build
```

## Architecture

```
DepoAudio
├── src/                        # React frontend
│   ├── components/
│   │   ├── Convert/            # Conversion UI + AI Enhance panel
│   │   ├── Player/             # Global audio player
│   │   ├── Library/            # Case library + CAT detection
│   │   └── common/             # Shared components
│   ├── hooks/                  # Custom hooks (theme, prefs, conversion)
│   └── App.jsx                 # Main app shell (4-tab layout)
├── src-tauri/                  # Rust backend (Tauri v2)
│   ├── src/
│   │   ├── analysis.rs         # Audio analysis + Smart Turn ONNX inference
│   │   ├── catdetect.rs        # Court reporting software detection
│   │   ├── commands.rs         # Tauri command handlers
│   │   ├── conversion.rs       # Two-step pipeline (Rust AI → FFmpeg)
│   │   ├── denoise.rs          # RNNoise + DeepFilterNet3 denoising
│   │   ├── enhance.rs          # FlashSR bandwidth extension
│   │   ├── ffmpeg.rs           # FFmpeg sidecar + filter chain
│   │   ├── models.rs           # ONNX model loader + hardware detection
│   │   ├── scoring.rs          # DNSMOS quality scoring
│   │   ├── speakers.rs         # Speaker count detection
│   │   ├── types.rs            # Shared type definitions
│   │   └── persistence.rs      # Library & prefs storage
│   ├── resources/models/       # Bundled ONNX models (~57MB)
│   └── binaries/               # FFmpeg/FFprobe sidecars (not committed)
└── .github/workflows/          # CI/CD (release builds)
```

**Stack:** Tauri 2 · Rust · React 19 · Vite · FFmpeg · ONNX Runtime · nnnoiseless

### AI Models

| Model | Size | Purpose |
|-------|------|---------|
| Smart Turn v3 (int8) | 8.2 MB | Speaker turn detection |
| FlashSR | 487 KB | Bandwidth extension (16→48 kHz) |
| DeepFilterNet3 (3 files) | 8.2 MB | Premium speech denoising |
| DNSMOS | 297 KB | Audio quality scoring |
| Speaker segmentation (int8) | 1.5 MB | Speaker count detection |
| Speaker embedding | 38 MB | Speaker identification |

## Releasing

Push a version tag to trigger automated builds:

```bash
git tag v1.0.0
git push origin v1.0.0
```

GitHub Actions builds installers for macOS (ARM64 + Intel) and Windows (x64), then creates a draft release with all assets.

## License

[MIT](LICENSE)
