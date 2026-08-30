#!/usr/bin/env bash
set -euo pipefail

: "${RUNNER_TEMP:?RUNNER_TEMP is required}"
: "${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}"
FTR_FIXTURE_PATH="$RUNNER_TEMP/depoaudio-ftr-smoke/ftr-smoke.trm"

bundle_root="$GITHUB_WORKSPACE/src-tauri/target/universal-apple-darwin/release/bundle"
dmg=$(find "$bundle_root" -type f -name '*.dmg' -print -quit)
app_archive=$(find "$bundle_root" -type f -name '*.app.tar.gz' -print -quit)
test -n "$dmg" && test -n "$app_archive" || {
  echo 'ERROR: the downloadable macOS artifact pair is incomplete'
  exit 1
}

mount_point="$RUNNER_TEMP/depoaudio-dmg-smoke"
archive_root="$RUNNER_TEMP/depoaudio-app-archive-smoke"
mkdir -p "$mount_point" "$archive_root"
mounted=false
cleanup() {
  node scripts/ftr-smoke-fixture.mjs --clean >/dev/null 2>&1 || true
  rm -f -- "$RUNNER_TEMP"/packaged-macos-*-smoke.*
  if [ "$mounted" = 'true' ]; then
    hdiutil detach "$mount_point" -quiet || true
  fi
}
trap cleanup EXIT

node scripts/ftr-smoke-fixture.mjs --generate \
  --ffmpeg "$GITHUB_WORKSPACE/src-tauri/binaries/ffmpeg-universal-apple-darwin"
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount_point" -quiet
mounted=true
tar xzf "$app_archive" -C "$archive_root"

dmg_app=$(find "$mount_point" -maxdepth 2 -type d -name 'DepoAudio.app' -print -quit)
archive_app=$(find "$archive_root" -maxdepth 3 -type d -name 'DepoAudio.app' -print -quit)
test -n "$dmg_app" && test -n "$archive_app" || {
  echo 'ERROR: a downloadable macOS container has no DepoAudio.app'
  exit 1
}

verify_universal() {
  local file="$1"
  local label="$2"
  local archs
  archs=$(lipo -archs "$file")
  case "$archs" in
    *x86_64*arm64* | *arm64*x86_64*) ;;
    *)
      echo "ERROR: $label is not universal (got: $archs)"
      exit 1
      ;;
  esac
}

smoke_app() {
  local app="$1"
  local label="$2"
  local label_key
  label_key=$(printf '%s' "$label" | tr '[:upper:]' '[:lower:]')
  local executable="$app/Contents/MacOS/depo-audio"
  local ffmpeg="$app/Contents/MacOS/ffmpeg"
  local ffprobe="$app/Contents/MacOS/ffprobe"

  test -x "$executable" && test -x "$ffmpeg" && test -x "$ffprobe" || {
    echo "ERROR: $label app has missing or non-executable packaged native payloads"
    exit 1
  }
  if find "$app" -type f \( -iname '*.onnx' -o -iname 'libonnxruntime*.dylib' \) -print -quit | grep -q .; then
    echo "ERROR: $label app contains forbidden learned-model material"
    exit 1
  fi
  verify_universal "$executable" "$label app executable"
  verify_universal "$ffmpeg" "$label FFmpeg"
  verify_universal "$ffprobe" "$label FFprobe"

  codesign --verify --deep --strict --verbose=2 "$app"
  if [ "${EXPECT_PLATFORM_SIGNING:-false}" = 'true' ]; then
    codesign --display --verbose=4 "$app"
    xcrun stapler validate "$app"
    spctl --assess --type execute --verbose=4 "$app"
  fi

  local probe_json
  probe_json=$("$ffprobe" -v error -select_streams a:0 \
    -show_entries stream=codec_type,codec_name,channels -of json "$FTR_FIXTURE_PATH")
  printf '%s' "$probe_json" | grep -q '"codec_type": "audio"' || {
    echo "ERROR: $label FFprobe did not identify an audio stream"
    exit 1
  }
  printf '%s' "$probe_json" | grep -q '"codec_name": "ftr"' || {
    echo "ERROR: $label FFprobe did not identify the native FTR decoder"
    exit 1
  }

  "$ffmpeg" -hide_banner -v error -c:a ftr -t 5 \
    -i "$FTR_FIXTURE_PATH" -map 0:a:0 -f null -
  for spec in wav:pcm_s16le mp3:libmp3lame flac:flac opus:libopus m4a:aac; do
    local extension=${spec%%:*}
    local codec=${spec#*:}
    local output="$RUNNER_TEMP/packaged-macos-$label_key-smoke.$extension"
    "$ffmpeg" -hide_banner -v error -xerror -c:a ftr -t 1 \
      -i "$FTR_FIXTURE_PATH" -map 0:a:0 -ac 1 -c:a "$codec" -y "$output"
    test -s "$output" || {
      echo "ERROR: $label FFmpeg produced no $extension output"
      exit 1
    }
    rm -f -- "$output"
  done

  local stdout="$RUNNER_TEMP/depoaudio-$label_key-startup-stdout.txt"
  local stderr="$RUNNER_TEMP/depoaudio-$label_key-startup-stderr.txt"
  "$executable" >"$stdout" 2>"$stderr" &
  local pid=$!
  for _ in {1..24}; do
    sleep 0.5
    if ! kill -0 "$pid" 2>/dev/null; then
      local status=0
      wait "$pid" || status=$?
      echo "== $label packaged app stdout =="
      cat "$stdout" || true
      echo "== $label packaged app stderr =="
      cat "$stderr" || true
      echo "ERROR: $label packaged app exited during startup with code $status"
      exit 1
    fi
  done
  kill "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || true
}

smoke_app "$dmg_app" 'DMG'
smoke_app "$archive_app" 'Archive'
