#!/bin/bash
set -e

# Setup script for local DepoAudio development.
# Copies the FFmpeg sidecars required for the current platform.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TAURI_DIR="$PROJECT_DIR/src-tauri"

# Detect platform
ARCH=$(uname -m)
OS=$(uname -s)

if [ "$OS" = "Darwin" ]; then
  if [ "$ARCH" = "arm64" ]; then
    TARGET="aarch64-apple-darwin"
  else
    TARGET="x86_64-apple-darwin"
  fi
elif [ "$OS" = "Linux" ]; then
  TARGET="x86_64-unknown-linux-gnu"
else
  echo "Unsupported OS: $OS (use Windows setup manually)"
  exit 1
fi

echo "=== DepoAudio Dev Setup ==="
echo "Platform: $OS $ARCH ($TARGET)"
echo ""

# ── FFmpeg sidecars ──────────────────────────────────────────────────────────
echo "--- FFmpeg Sidecars ---"
mkdir -p "$TAURI_DIR/binaries"

if command -v ffmpeg &>/dev/null; then
  FFMPEG_PATH=$(which ffmpeg)
  FFPROBE_PATH=$(which ffprobe)
  echo "Found FFmpeg: $FFMPEG_PATH"
  cp "$FFMPEG_PATH" "$TAURI_DIR/binaries/ffmpeg-$TARGET"
  cp "$FFPROBE_PATH" "$TAURI_DIR/binaries/ffprobe-$TARGET"
  chmod +x "$TAURI_DIR/binaries/ffmpeg-$TARGET" "$TAURI_DIR/binaries/ffprobe-$TARGET"
  echo "Copied to src-tauri/binaries/"
else
  echo "FFmpeg not found. Install with: brew install ffmpeg"
  exit 1
fi

echo ""
echo "--- Done ---"
echo "You can now run: npm run tauri dev"
