#!/usr/bin/env bash

# Build the exact LGPL FFmpeg sidecars used by the Windows x64 release.
# This cross-compiles the same pinned FFmpeg, LAME and Opus sources as
# scripts/build-ffmpeg-macos.sh, on Ubuntu with the win32-thread MinGW-w64
# GCC, statically, so the two programs need nothing beyond Windows itself.
# The app launches them as separate processes; no FFmpeg library is linked
# into DepoAudio.
#
# Output (in OUTPUT_DIR, default $RUNNER_TEMP/depoaudio-ffmpeg-windows):
#   binaries/ffmpeg-x86_64-pc-windows-msvc.exe
#   binaries/ffprobe-x86_64-pc-windows-msvc.exe
#   notices/   the notice set the Windows app ships in third-party/ffmpeg
#   build-ffmpeg-windows.sh   this script, for the corresponding-source archive
#
# The programs cannot run on the Linux build machine, so this script gates the
# configuration FFmpeg recorded at build time. The Windows release leg runs the
# built programs and repeats the gates against what they report.

set -euo pipefail

required_env=(
  FFMPEG_SOURCE_URL FFMPEG_SOURCE_SIZE FFMPEG_SOURCE_SHA256
  LAME_SOURCE_URL LAME_SOURCE_SIZE LAME_SOURCE_SHA256
  OPUS_SOURCE_URL OPUS_SOURCE_SIZE OPUS_SOURCE_SHA256
)
for name in "${required_env[@]}"; do
  test -n "${!name:-}" || { echo "ERROR: missing required environment variable $name" >&2; exit 1; }
done

cross=x86_64-w64-mingw32
for tool in curl sha256sum tar make nasm pkg-config "$cross-gcc" "$cross-objdump" "$cross-strip"; do
  command -v "$tool" >/dev/null || { echo "ERROR: required build tool is unavailable: $tool" >&2; exit 1; }
done

# The win32 thread model makes FFmpeg use native Windows threads, so no
# libwinpthread is linked or has to be noticed.
thread_model=$("$cross-gcc" -v 2>&1 | sed -n 's/^Thread model: //p')
test "$thread_model" = "win32" || {
  echo "ERROR: $cross-gcc uses thread model '$thread_model'; the win32 variant is required" >&2
  exit 1
}

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work_parent=${RUNNER_TEMP:-"$repo_root/.tmp"}
mkdir -p "$work_parent"
output_dir=${OUTPUT_DIR:-"$work_parent/depoaudio-ffmpeg-windows"}
test ! -e "$output_dir" || { echo "ERROR: output directory already exists: $output_dir" >&2; exit 1; }
work_root=$(mktemp -d "$work_parent/depoaudio-ffmpeg-windows-build.XXXXXX")
trap 'rm -rf "$work_root"' EXIT

download_and_verify() {
  local url=$1 output=$2 expected_size=$3 expected_sha256=$4
  curl -fL --retry 5 --retry-all-errors --retry-delay 2 "$url" -o "$output"
  local actual_size actual_sha256
  actual_size=$(stat -c%s "$output")
  test "$actual_size" = "$expected_size" || {
    echo "ERROR: $(basename "$output") size $actual_size does not match $expected_size" >&2
    exit 1
  }
  actual_sha256=$(sha256sum "$output" | awk '{print $1}')
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

jobs=$(nproc)
dependency_prefix="$work_root/dependencies"
install_prefix="$work_root/output"
mkdir -p "$dependency_prefix" "$install_prefix"
common_cflags="-O2"

lame_source="$work_root/lame"
extract_source "$sources/lame.tar.gz" "$lame_source"
(
  cd "$lame_source"
  env CC="$cross-gcc" CFLAGS="$common_cflags" \
    ./configure \
      --host="$cross" \
      --prefix="$dependency_prefix" \
      --enable-static \
      --disable-shared \
      --disable-frontend \
      --disable-gtktest
  make -j"$jobs"
  make install
)

opus_source="$work_root/opus"
extract_source "$sources/opus.tar.gz" "$opus_source"
(
  cd "$opus_source"
  env CC="$cross-gcc" CFLAGS="$common_cflags" \
    ./configure \
      --host="$cross" \
      --prefix="$dependency_prefix" \
      --enable-static \
      --disable-shared
  make -j"$jobs"
  make install
)

ffmpeg_source="$work_root/ffmpeg"
extract_source "$sources/ffmpeg.tar.xz" "$ffmpeg_source"
(
  cd "$ffmpeg_source"
  env PKG_CONFIG_PATH="$dependency_prefix/lib/pkgconfig" PKG_CONFIG_LIBDIR="$dependency_prefix/lib/pkgconfig" ./configure \
    --prefix="$install_prefix" \
    --enable-cross-compile \
    --cross-prefix="$cross-" \
    --target-os=mingw32 \
    --arch=x86_64 \
    --pkg-config=pkg-config \
    --pkg-config-flags=--static \
    --extra-cflags="-I$dependency_prefix/include" \
    --extra-ldflags="-static -static-libgcc -L$dependency_prefix/lib" \
    --disable-shared \
    --enable-static \
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

ffmpeg="$install_prefix/bin/ffmpeg.exe"
ffprobe="$install_prefix/bin/ffprobe.exe"
test -f "$ffmpeg" && test -f "$ffprobe" || { echo "ERROR: the Windows build produced no sidecars" >&2; exit 1; }

# FFmpeg records the configuration that `ffmpeg -version` prints and the
# license it computed in config.h, and every enabled component in
# config_components.h. Gate those, since the .exe cannot run here.
config_h="$ffmpeg_source/config.h"
components_h="$ffmpeg_source/config_components.h"
test -f "$config_h" && test -f "$components_h" || { echo "ERROR: FFmpeg left no config.h" >&2; exit 1; }
configuration=$(sed -n 's/^#define FFMPEG_CONFIGURATION "\(.*\)"$/\1/p' "$config_h")
license=$(sed -n 's/^#define FFMPEG_LICENSE "\(.*\)"$/\1/p' "$config_h")
test -n "$configuration" && test -n "$license" || { echo "ERROR: config.h has no configuration or license" >&2; exit 1; }
! grep -q -- '--enable-gpl' <<<"$configuration" || { echo "ERROR: Windows FFmpeg enabled GPL code" >&2; exit 1; }
! grep -q -- '--enable-nonfree' <<<"$configuration" || { echo "ERROR: Windows FFmpeg enabled nonfree code" >&2; exit 1; }
grep -qx '#define CONFIG_GPL 0' "$config_h" || { echo "ERROR: Windows FFmpeg config has GPL enabled" >&2; exit 1; }
grep -qx '#define CONFIG_NONFREE 0' "$config_h" || { echo "ERROR: Windows FFmpeg config has nonfree enabled" >&2; exit 1; }
! grep -qi 'unredistributable' <<<"$license" || { echo "ERROR: Windows FFmpeg is unredistributable" >&2; exit 1; }
case "$license" in
  'LGPL version 2.1 or later' | 'LGPL version 3 or later') : ;;
  *) echo "ERROR: Windows FFmpeg license is '$license', not LGPL" >&2; exit 1 ;;
esac

require_component() {
  local kind=$1 name=$2 macro
  macro="CONFIG_$(tr '[:lower:]' '[:upper:]' <<<"${name}_${kind}")"
  grep -qx "#define $macro 1" "$components_h" || {
    echo "ERROR: Windows FFmpeg lacks $(tr '[:upper:]' '[:lower:]' <<<"$kind") $name" >&2; exit 1;
  }
}
# The same component lists scripts/build-ffmpeg-macos.sh gates on (see the
# comments there for why each is needed). The release leg repeats them against
# `ffmpeg.exe -decoders`, `-encoders`, `-filters` and `-formats` on Windows.
for decoder in ftr libopus aac flac pcm_s16le mp2 ac3 wmav2 wmapro wmavoice pcm_dvd pcm_bluray gsm_ms adpcm_ima_wav adpcm_ms pcm_alaw pcm_mulaw; do
  require_component DECODER "$decoder"
done
for decoder in pcm_alaw pcm_mulaw pcm_u8 pcm_s24le pcm_s32le adpcm_ima_wav gsm_ms mp3float; do
  require_component DECODER "$decoder"
done
for encoder in libmp3lame libopus aac flac pcm_s16le; do
  require_component ENCODER "$encoder"
done
for filter in adeclip highpass volume loudnorm afade silenceremove alimiter asplit pan aresample ebur128 amerge aformat astats atrim asetpts; do
  require_component FILTER "$filter"
done
# `-formats` names a muxer/demuxer pair; these are their component names.
for demuxer in wav mp3 flac ogg mov matroska avi mpegps mpegts asf; do
  require_component DEMUXER "$demuxer"
done
for muxer in wav mp3 flac opus ogg mov ipod; do
  require_component MUXER "$muxer"
done
require_component PARSER ftr

# Only Windows' own DLLs may be imported: no MinGW, GCC or codec DLL ships.
system_dlls=' advapi32.dll bcrypt.dll kernel32.dll msvcrt.dll ole32.dll psapi.dll shell32.dll user32.dll ws2_32.dll secur32.dll '
for binary in "$ffmpeg" "$ffprobe"; do
  imports=$("$cross-objdump" -p "$binary" | sed -n 's/^[[:space:]]*DLL Name: //p' | tr '[:upper:]' '[:lower:]')
  test -n "$imports" || { echo "ERROR: $(basename "$binary") lists no DLL imports" >&2; exit 1; }
  for dll in $imports; do
    case "$dll" in
      api-ms-win-*) continue ;;
    esac
    case "$system_dlls" in
      *" $dll "*) : ;;
      *) echo "ERROR: $(basename "$binary") imports a non-system DLL: $dll" >&2; exit 1 ;;
    esac
  done
done

mkdir -p "$output_dir/binaries" "$output_dir/notices"
for sidecar in ffmpeg ffprobe; do
  cp "$install_prefix/bin/$sidecar.exe" "$output_dir/binaries/$sidecar-x86_64-pc-windows-msvc.exe"
  "$cross-strip" "$output_dir/binaries/$sidecar-x86_64-pc-windows-msvc.exe"
done

notice_dir="$output_dir/notices"
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
# Static linking puts the MinGW-w64 C runtime startup code and libgcc in both
# programs. Their terms (public domain / permissive, and GPLv3 with the GCC
# Runtime Library Exception) travel with them.
for package in mingw-w64-common gcc-mingw-w64-base; do
  copyright="/usr/share/doc/$package/copyright"
  test -s "$copyright" || { echo "ERROR: $copyright is missing" >&2; exit 1; }
done
cp /usr/share/doc/mingw-w64-common/copyright "$notice_dir/MinGW-w64-COPYRIGHT.txt"
cp /usr/share/doc/gcc-mingw-w64-base/copyright "$notice_dir/GCC-MinGW-w64-COPYRIGHT.txt"
grep -q 'GCC RUNTIME LIBRARY EXCEPTION' "$notice_dir/GCC-MinGW-w64-COPYRIGHT.txt" || {
  echo "ERROR: the GCC notice lacks the GCC Runtime Library Exception" >&2; exit 1;
}

{
  printf 'configuration: %s\n' "$configuration"
  printf 'License: %s\n' "$license"
} > "$notice_dir/BUILD-CONFIGURATION.txt"

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
Build script: scripts/build-ffmpeg-windows.sh in the DepoAudio source release
Configuration: BUILD-CONFIGURATION.txt
Toolchain: BUILD-TOOLCHAIN.txt (MinGW-w64 GCC, win32 threads, static)
This file records provenance; it is not a legal compliance determination.
EOF

{
  "$cross-gcc" --version | head -1
  printf 'Thread model: %s\n' "$thread_model"
  "$cross-objdump" --version | head -1
  if command -v dpkg-query >/dev/null; then
    dpkg-query -W -f='${Package} ${Version}\n' \
      gcc-mingw-w64-x86-64-win32 mingw-w64-x86-64-dev binutils-mingw-w64-x86-64 2>/dev/null || true
  fi
  printf 'nasm: '; nasm -v
  printf 'pkg-config: '; pkg-config --version
  make --version | head -1
} > "$notice_dir/BUILD-TOOLCHAIN.txt"

cp "$repo_root/scripts/build-ffmpeg-windows.sh" "$output_dir/build-ffmpeg-windows.sh"
(cd "$output_dir" && find binaries notices build-ffmpeg-windows.sh -type f -print0 | sort -z | xargs -0 sha256sum) \
  > "$work_root/SHA256SUMS-ffmpeg-windows.txt"
mv "$work_root/SHA256SUMS-ffmpeg-windows.txt" "$output_dir/SHA256SUMS-ffmpeg-windows.txt"

echo 'Windows LGPL FFmpeg sidecars built and verified.'
