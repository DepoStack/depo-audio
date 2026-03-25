#!/bin/bash
set -e

# Setup script for local DepoAudio development.
# Downloads FFmpeg sidecars and ONNX Runtime for the current platform.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
TAURI_DIR="$PROJECT_DIR/src-tauri"

# Detect platform
ARCH=$(uname -m)
OS=$(uname -s)

if [ "$OS" = "Darwin" ]; then
  if [ "$ARCH" = "arm64" ]; then
    TARGET="aarch64-apple-darwin"
    ORT_URL="https://github.com/microsoft/onnxruntime/releases/download/v1.21.1/onnxruntime-osx-arm64-1.21.1.tgz"
  else
    TARGET="x86_64-apple-darwin"
    ORT_URL="https://github.com/microsoft/onnxruntime/releases/download/v1.21.1/onnxruntime-osx-x86_64-1.21.1.tgz"
  fi
  ORT_LIB="libonnxruntime.1.21.1.dylib"
  ORT_DEST="libonnxruntime.dylib"
elif [ "$OS" = "Linux" ]; then
  TARGET="x86_64-unknown-linux-gnu"
  ORT_URL="https://github.com/microsoft/onnxruntime/releases/download/v1.21.1/onnxruntime-linux-x64-1.21.1.tgz"
  ORT_LIB="libonnxruntime.so.1.21.1"
  ORT_DEST="libonnxruntime.so"
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

# ── ONNX Runtime ─────────────────────────────────────────────────────────────
echo ""
echo "--- ONNX Runtime ---"
ORT_DIR="$TAURI_DIR/resources/onnxruntime"
mkdir -p "$ORT_DIR"

if [ -f "$ORT_DIR/$ORT_DEST" ]; then
  echo "Already present: $ORT_DIR/$ORT_DEST"
else
  echo "Downloading ONNX Runtime..."
  TMPDIR=$(mktemp -d)
  curl -sL "$ORT_URL" -o "$TMPDIR/ort.tgz"
  tar xzf "$TMPDIR/ort.tgz" -C "$TMPDIR"
  ORT_EXTRACT=$(ls -d "$TMPDIR"/onnxruntime-*)
  cp "$ORT_EXTRACT/lib/$ORT_LIB" "$ORT_DIR/$ORT_DEST"
  rm -rf "$TMPDIR"
  echo "Installed: $ORT_DIR/$ORT_DEST"
fi

echo ""
echo "--- Done ---"
echo "You can now run: npm run tauri dev"
