# Run the source-built Windows FFmpeg sidecars and gate what they report.
# scripts/build-ffmpeg-windows.sh gates the configuration recorded at build
# time on Linux, where the programs cannot run. This repeats the same gates
# on Windows against the programs themselves, before they enter an installer.

param(
  [Parameter(Mandatory = $true)][string]$FFmpeg,
  [Parameter(Mandatory = $true)][string]$FFprobe,
  [Parameter(Mandatory = $true)][string]$BuildConfiguration
)

$ErrorActionPreference = 'Stop'

function Invoke-Sidecar([string]$Program, [string[]]$Arguments) {
  $output = (& $Program @Arguments) -join "`n"
  if ($LASTEXITCODE -ne 0) { throw "$(Split-Path -Leaf $Program) $($Arguments -join ' ') failed" }
  return $output
}

foreach ($program in @($FFmpeg, $FFprobe)) {
  if (-not (Test-Path -LiteralPath $program -PathType Leaf)) { throw "Missing sidecar: $program" }
}
if (-not (Test-Path -LiteralPath $BuildConfiguration -PathType Leaf)) {
  throw "Missing recorded build configuration: $BuildConfiguration"
}

$version = Invoke-Sidecar $FFmpeg @('-version')
$probeVersion = Invoke-Sidecar $FFprobe @('-version')
foreach ($report in @($version, $probeVersion)) {
  if ($report -match '--enable-gpl') { throw 'Windows FFmpeg enabled GPL code' }
  if ($report -match '--enable-nonfree') { throw 'Windows FFmpeg enabled nonfree code' }
  if ($report -match '(?i)unredistributable') { throw 'Windows FFmpeg is unredistributable' }
}

# Both programs must report exactly the configuration the Linux build
# recorded. -cne compares case-sensitively; PowerShell's -ne ignores case.
$recorded = Get-Content -LiteralPath $BuildConfiguration -Raw
$recordedConfiguration = ($recorded -split "`r?`n" | Where-Object { $_ -clike 'configuration: *' } | Select-Object -First 1)
if (-not $recordedConfiguration) { throw "The recorded build configuration has no configuration line: $BuildConfiguration" }
foreach ($pair in @(@('ffmpeg.exe', $version), @('ffprobe.exe', $probeVersion))) {
  $reportedConfiguration = ($pair[1] -split "`r?`n" | Where-Object { $_ -clike 'configuration: *' } | Select-Object -First 1)
  if ($recordedConfiguration -cne $reportedConfiguration) {
    throw "$($pair[0]) reports a different configuration than the build recorded:`n  recorded: $recordedConfiguration`n  reported: $reportedConfiguration"
  }
}

$decoders = Invoke-Sidecar $FFmpeg @('-hide_banner', '-decoders')
$encoders = Invoke-Sidecar $FFmpeg @('-hide_banner', '-encoders')
$filters = Invoke-Sidecar $FFmpeg @('-hide_banner', '-filters')
$formats = Invoke-Sidecar $FFmpeg @('-hide_banner', '-formats')

# The same lists scripts/build-ffmpeg-macos.sh gates the macOS sidecars on.
$requiredDecoders = @(
  'ftr', 'libopus', 'aac', 'flac', 'pcm_s16le', 'mp2', 'ac3', 'wmav2', 'wmapro', 'wmavoice',
  'pcm_dvd', 'pcm_bluray', 'gsm_ms', 'adpcm_ima_wav', 'adpcm_ms', 'pcm_alaw', 'pcm_mulaw',
  'pcm_u8', 'pcm_s24le', 'pcm_s32le', 'mp3float'
)
foreach ($decoder in $requiredDecoders) {
  if ($decoders -notmatch "\s$decoder\s") { throw "Windows FFmpeg lacks decoder $decoder" }
}
foreach ($encoder in @('libmp3lame', 'libopus', 'aac', 'flac', 'pcm_s16le')) {
  if ($encoders -notmatch "\s$encoder\s") { throw "Windows FFmpeg lacks encoder $encoder" }
}
$requiredFilters = @(
  'adeclip', 'highpass', 'volume', 'loudnorm', 'afade', 'silenceremove', 'alimiter', 'asplit',
  'pan', 'aresample', 'ebur128', 'amerge', 'aformat', 'astats', 'atrim', 'asetpts'
)
foreach ($filter in $requiredFilters) {
  if ($filters -notmatch "\s$filter\s") { throw "Windows FFmpeg lacks filter $filter" }
}
foreach ($format in @('wav', 'mp3', 'flac', 'opus', 'mov', 'ogg', 'matroska', 'avi', 'mpeg', 'mpegts', 'asf')) {
  if ($formats -notmatch "\s$format(,|\s)") { throw "Windows FFmpeg lacks format $format" }
}

Write-Output 'Source-built Windows FFmpeg sidecars report the reviewed LGPL configuration.'
