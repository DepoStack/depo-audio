#!/usr/bin/env bash

# Build the exact LGPL FFmpeg sidecars used by the universal macOS release.
# The app launches these programs as separate processes; no FFmpeg library is
# linked into DepoAudio. All source archives are versioned, size/hash checked,
# and their applicable license texts are preserved in the app bundle.

set -euo pipefail

required_env=(
  FFMPEG_SOURCE_URL FFMPEG_SOURCE_SIZE FFMPEG_SOURCE_SHA256
  LAME_SOURCE_URL LAME_SOURCE_SIZE LAME_SOURCE_SHA256
  OPUS_SOURCE_URL OPUS_SOURCE_SIZE OPUS_SOURCE_SHA256
)
for name in "${required_env[@]}"; do
  test -n "${!name:-}" || { echo "ERROR: missing required environment variable $name" >&2; exit 1; }
done

for tool in curl shasum tar make clang lipo nasm otool pkg-config; do
  command -v "$tool" >/dev/null || { echo "ERROR: required build tool is unavailable: $tool" >&2; exit 1; }
done

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work_parent=${RUNNER_TEMP:-"$repo_root/.tmp"}
work_root=$(mktemp -d "$work_parent/depoaudio-ffmpeg-macos.XXXXXX")
trap 'rm -rf "$work_root"' EXIT

download_and_verify() {
  local url=$1 output=$2 expected_size=$3 expected_sha256=$4
  curl -fL --retry 5 --retry-all-errors --retry-delay 2 "$url" -o "$output"
  local actual_size actual_sha256
  actual_size=$(stat -f%z "$output")
  test "$actual_size" = "$expected_size" || {
    echo "ERROR: $(basename "$output") size $actual_size does not match $expected_size" >&2
    exit 1
  }
  actual_sha256=$(shasum -a 256 "$output" | awk '{print $1}')
  test "$actual_sha256" = "$expected_sha256" || {
    echo "ERROR: $(basename "$output") SHA-256 $actual_sha256 does not match $expected_sha256" >&2
    exit 1
  }
}

sources="$work_root/sources"
mkdir -p "$sources"
download_and_verify "$FFMPEG_SOURCE_URL" "$sources/ffmpeg.tar.xz" "$FFMPEG_SOURCE_SIZE" "$FFMPEG_SOURCE_SHA256"
download_and_verify "$LAME_SOURCE_URL" "$sources/lame.tar.gz" "$LAME_SOURCE_SIZE" "$LAME_SOURCE_SHA256"
download_and_verify "$OPUS_SOURCE_URL" "$sources/opus.tar.gz" "$OPUS_SOURCE_SIZE" "$OPUS_SOURCE_SHA256"

extract_source() {
  local archive=$1 destination=$2
  mkdir -p "$destination"
  tar -xf "$archive" --strip-components=1 -C "$destination"
}

jobs=$(sysctl -n hw.ncpu)
minimum_macos=13.3

build_arch() {
  local arch=$1 clang_target=$2 host_target=$3 tauri_triple=$4
  local arch_root="$work_root/build-$arch"
  local dependency_prefix="$arch_root/dependencies"
  local output_prefix="$arch_root/output"
  local common_cflags="-O2 -fPIC -target $clang_target -mmacosx-version-min=$minimum_macos"
  mkdir -p "$dependency_prefix" "$output_prefix"

  local lame_source="$arch_root/lame"
  extract_source "$sources/lame.tar.gz" "$lame_source"
  (
    cd "$lame_source"
    env \
      CC="/usr/bin/clang -target $clang_target" \
      CFLAGS="$common_cflags" \
      ./configure \
        --host="$host_target" \
        --prefix="$dependency_prefix" \
        --enable-static \
        --disable-shared \
        --disable-frontend \
        --disable-gtktest
    make -j"$jobs"
    make install
  )

  local opus_source="$arch_root/opus"
  extract_source "$sources/opus.tar.gz" "$opus_source"
  (
    cd "$opus_source"
    env \
      CC="/usr/bin/clang -target $clang_target" \
      CFLAGS="$common_cflags" \
      ./configure \
        --host="$host_target" \
        --prefix="$dependency_prefix" \
        --enable-static \
        --disable-shared
    make -j"$jobs"
    make install
  )

  local ffmpeg_source="$arch_root/ffmpeg"
  extract_source "$sources/ffmpeg.tar.xz" "$ffmpeg_source"
  (
    cd "$ffmpeg_source"
    env PKG_CONFIG_PATH="$dependency_prefix/lib/pkgconfig" ./configure \
      --prefix="$output_prefix" \
      --cc=/usr/bin/clang \
      --pkg-config-flags=--static \
      --enable-cross-compile \
      --target-os=darwin \
      --arch="$arch" \
      --extra-cflags="-target $clang_target -mmacosx-version-min=$minimum_macos -I$dependency_prefix/include" \
      --extra-ldflags="-target $clang_target -mmacosx-version-min=$minimum_macos -L$dependency_prefix/lib" \
      --disable-shared \
      --enable-static \
      --enable-pic \
      --enable-runtime-cpudetect \
      --disable-gpl \
      --disable-nonfree \
      --disable-autodetect \
      --disable-network \
      --disable-debug \
      --disable-doc \
      --disable-avdevice \
      --disable-ffplay \
      --disable-sdl2 \
      --enable-libmp3lame \
      --enable-libopus \
      --enable-decoder=ftr \
      --enable-parser=ftr \
      --enable-encoder=libmp3lame \
      --enable-encoder=libopus
    make -j"$jobs"
    make install
  )

  local ffmpeg="$output_prefix/bin/ffmpeg"
  local ffprobe="$output_prefix/bin/ffprobe"
  test -x "$ffmpeg" && test -x "$ffprobe" || { echo "ERROR: $arch build produced no sidecars" >&2; exit 1; }

  local version decoders encoders filters formats
  version=$("$ffmpeg" -version)
  decoders=$("$ffmpeg" -hide_banner -decoders)
  encoders=$("$ffmpeg" -hide_banner -encoders)
  filters=$("$ffmpeg" -hide_banner -filters)
  formats=$("$ffmpeg" -hide_banner -formats)
  ! grep -q -- '--enable-gpl' <<<"$version" || { echo "ERROR: $arch FFmpeg enabled GPL code" >&2; exit 1; }
  ! grep -q -- '--enable-nonfree' <<<"$version" || { echo "ERROR: $arch FFmpeg enabled nonfree code" >&2; exit 1; }
  ! grep -qi 'unredistributable' <<<"$version" || { echo "ERROR: $arch FFmpeg is unredistributable" >&2; exit 1; }

  for decoder in ftr aac flac pcm_s16le; do
    grep -Eq "[[:space:]]${decoder}[[:space:]]" <<<"$decoders" || {
      echo "ERROR: $arch FFmpeg lacks decoder $decoder" >&2; exit 1;
    }
  done
  for encoder in libmp3lame libopus aac flac pcm_s16le; do
    grep -Eq "[[:space:]]${encoder}[[:space:]]" <<<"$encoders" || {
      echo "ERROR: $arch FFmpeg lacks encoder $encoder" >&2; exit 1;
    }
  done
  for filter in adeclip highpass volume loudnorm afade silenceremove alimiter asplit pan aresample ebur128; do
    grep -Eq "[[:space:]]${filter}[[:space:]]" <<<"$filters" || {
      echo "ERROR: $arch FFmpeg lacks filter $filter" >&2; exit 1;
    }
  done
  for format in wav mp3 flac opus mov ogg matroska avi; do
    grep -Eq "[[:space:]]${format}(,|[[:space:]])" <<<"$formats" || {
      echo "ERROR: $arch FFmpeg lacks format $format" >&2; exit 1;
    }
  done

  for sidecar in ffmpeg ffprobe; do
    cp "$output_prefix/bin/$sidecar" "$repo_root/src-tauri/binaries/$sidecar-$tauri_triple"
    chmod +x "$repo_root/src-tauri/binaries/$sidecar-$tauri_triple"
  done

  mkdir -p "$repo_root/src-tauri/resources/third-party/ffmpeg"
  printf '%s\n' "$version" > "$repo_root/src-tauri/resources/third-party/ffmpeg/BUILD-CONFIGURATION-$arch.txt"
}

mkdir -p "$repo_root/src-tauri/binaries"
build_arch arm64 arm64-apple-macos13.3 aarch64-apple-darwin aarch64-apple-darwin
build_arch x86_64 x86_64-apple-macos13.3 x86_64-apple-darwin x86_64-apple-darwin

for sidecar in ffmpeg ffprobe; do
  lipo -create \
    "$repo_root/src-tauri/binaries/$sidecar-aarch64-apple-darwin" \
    "$repo_root/src-tauri/binaries/$sidecar-x86_64-apple-darwin" \
    -output "$repo_root/src-tauri/binaries/$sidecar-universal-apple-darwin"
  chmod +x "$repo_root/src-tauri/binaries/$sidecar-universal-apple-darwin"
  archs=$(lipo -archs "$repo_root/src-tauri/binaries/$sidecar-universal-apple-darwin")
  case "$archs" in
    *x86_64*arm64*|*arm64*x86_64*) : ;;
    *) echo "ERROR: $sidecar universal sidecar has unexpected architectures: $archs" >&2; exit 1 ;;
  esac
done

notice_dir="$repo_root/src-tauri/resources/third-party/ffmpeg"
license_sources="$work_root/license-sources"
extract_source "$sources/ffmpeg.tar.xz" "$license_sources/ffmpeg"
extract_source "$sources/lame.tar.gz" "$license_sources/lame"
extract_source "$sources/opus.tar.gz" "$license_sources/opus"
cp "$license_sources/ffmpeg/LICENSE.md" "$notice_dir/FFmpeg-LICENSE.md"
cp "$license_sources/ffmpeg/COPYING.LGPLv2.1" "$notice_dir/FFmpeg-COPYING.LGPLv2.1"
cp "$license_sources/ffmpeg/COPYING.LGPLv3" "$notice_dir/FFmpeg-COPYING.LGPLv3"
cp "$license_sources/lame/COPYING" "$notice_dir/LAME-COPYING"
cp "$license_sources/lame/LICENSE" "$notice_dir/LAME-LICENSE"
cp "$license_sources/opus/COPYING" "$notice_dir/Opus-COPYING"

cat > "$notice_dir/SOURCE.txt" <<EOF
FFmpeg source: $FFMPEG_SOURCE_URL
FFmpeg source size: $FFMPEG_SOURCE_SIZE
FFmpeg source SHA-256: $FFMPEG_SOURCE_SHA256
LAME source: $LAME_SOURCE_URL
LAME source size: $LAME_SOURCE_SIZE
LAME source SHA-256: $LAME_SOURCE_SHA256
Opus source: $OPUS_SOURCE_URL
Opus source size: $OPUS_SOURCE_SIZE
Opus source SHA-256: $OPUS_SOURCE_SHA256
Build script: scripts/build-ffmpeg-macos.sh in the DepoAudio source release
Configuration: BUILD-CONFIGURATION-arm64.txt and BUILD-CONFIGURATION-x86_64.txt
This file records provenance; it is not a legal compliance determination.
EOF

{
  clang --version
  printf 'nasm: '; nasm -v
  printf 'pkg-config: '; pkg-config --version
  make --version | head -1
} > "$notice_dir/BUILD-TOOLCHAIN.txt"

for binary in "$repo_root"/src-tauri/binaries/ff*-*-apple-darwin; do
  # A universal Mach-O repeats an unindented "binary (architecture ...):"
  # header for each slice. Inspect only indented dependency rows so the second
  # header cannot be mistaken for a non-system library.
  bad=$(otool -L "$binary" | awk '/^[[:space:]]/ { print $1 }' | grep -vE '^/usr/lib/|^/System/Library/' || true)
  if test -n "$bad"; then
    echo "ERROR: $(basename "$binary") has non-system dynamic dependencies:" >&2
    echo "$bad" >&2
    exit 1
  fi
done

# Keep corresponding source separate from the installed application. The
# release workflow publishes this directory as a versioned source archive next
# to the installers; the app itself receives the much smaller notice set above.
source_bundle="${RUNNER_TEMP:-$work_parent}/depoaudio-third-party-source"
test ! -e "$source_bundle" || {
  echo "ERROR: source-bundle directory already exists: $source_bundle" >&2
  exit 1
}
mkdir -p "$source_bundle/sources" "$source_bundle/notices"
cp "$sources/ffmpeg.tar.xz" "$source_bundle/sources/ffmpeg-7.1.5.tar.xz"
cp "$sources/lame.tar.gz" "$source_bundle/sources/lame-3.100.tar.gz"
cp "$sources/opus.tar.gz" "$source_bundle/sources/opus-1.6.1.tar.gz"
cp "$repo_root/scripts/build-ffmpeg-macos.sh" "$source_bundle/build-ffmpeg-macos.sh"
cp -R "$notice_dir/." "$source_bundle/notices/"
# The reviewed upstream archives are built without patches. Preserve an
# intentionally empty diff so the release carries an explicit, machine-checkable
# answer to FFmpeg's corresponding-source checklist rather than relying only on
# prose in the README.
: > "$source_bundle/changes.diff"
test ! -s "$source_bundle/changes.diff" || {
  echo "ERROR: no-patch changes.diff must be empty" >&2
  exit 1
}
cat > "$source_bundle/README.txt" <<'EOF'
DepoAudio macOS FFmpeg corresponding source

This archive contains the exact FFmpeg, LAME, and Opus source archives used to
build the macOS ffmpeg and ffprobe sidecars, the build script, both recorded
configurations, toolchain output, hashes, and applicable license texts.

No patches are applied to the source archives; changes.diff is intentionally
empty. The build script configures each architecture independently for the
DepoAudio macOS 13.3 minimum, then creates the universal sidecars with lipo. See
notices/SOURCE.txt for exact byte pins.
EOF

echo 'macOS LGPL FFmpeg sidecars built and verified.'
