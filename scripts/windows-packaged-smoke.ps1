Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Invoke-PackagedNativeSmoke {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $ffmpegMatches = @(Get-ChildItem -LiteralPath $Root -Recurse -Filter 'ffmpeg.exe' -File)
  $ffprobeMatches = @(Get-ChildItem -LiteralPath $Root -Recurse -Filter 'ffprobe.exe' -File)
  $ortMatches = @(Get-ChildItem -LiteralPath $Root -Recurse -Filter 'onnxruntime.dll' -File)
  if ($ffmpegMatches.Count -ne 1 -or $ffprobeMatches.Count -ne 1 -or $ortMatches.Count -ne 1) {
    throw "$Label packaged native payload is missing or duplicated"
  }

  $ffmpeg = $ffmpegMatches[0]
  $ffprobe = $ffprobeMatches[0]
  $probeJson = & $ffprobe.FullName -v error -c:a ftr -select_streams a:0 `
    -show_entries stream=codec_type,codec_name,channels -of json ftr-smoke.trm | Out-String
  if ($LASTEXITCODE -ne 0) { throw "$Label packaged FFprobe failed native FTR inspection" }
  $probe = $probeJson | ConvertFrom-Json
  if (@($probe.streams).Count -ne 1 -or $probe.streams[0].codec_type -ne 'audio') {
    throw "$Label packaged FFprobe did not identify exactly one audio stream"
  }

  & $ffmpeg.FullName -hide_banner -v error -c:a ftr -t 5 `
    -i ftr-smoke.trm -map 0:a:0 -f null NUL
  if ($LASTEXITCODE -ne 0) { throw "$Label packaged FFmpeg failed native FTR decoding" }

  $variants = @(
    @{ Extension = 'wav'; Codec = 'pcm_s16le' }
    @{ Extension = 'mp3'; Codec = 'libmp3lame' }
    @{ Extension = 'flac'; Codec = 'flac' }
    @{ Extension = 'opus'; Codec = 'libopus' }
    @{ Extension = 'm4a'; Codec = 'aac' }
  )
  foreach ($variant in $variants) {
    $output = Join-Path $env:RUNNER_TEMP "packaged-$($Label.ToLowerInvariant())-smoke.$($variant.Extension)"
    & $ffmpeg.FullName -hide_banner -v error -xerror -c:a ftr -t 1 `
      -i ftr-smoke.trm -map 0:a:0 -ac 1 -c:a $variant.Codec -y $output
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $output) -or (Get-Item $output).Length -eq 0) {
      throw "$Label packaged FFmpeg failed $($variant.Extension) encoding"
    }
    Remove-Item -LiteralPath $output -Force
  }

  $priorOrtPath = $env:ORT_DYLIB_PATH
  try {
    $env:ORT_DYLIB_PATH = $ortMatches[0].FullName
    cargo test --locked --manifest-path src-tauri/Cargo.toml `
      ort_loads_and_runs_silero_vad -- --ignored --nocapture
    if ($LASTEXITCODE -ne 0) { throw "$Label packaged ONNX Runtime smoke test failed" }
  }
  finally {
    $env:ORT_DYLIB_PATH = $priorOrtPath
  }
}

function Invoke-PackagedStartup {
  param(
    [Parameter(Mandatory = $true)][IO.FileInfo]$App,
    [Parameter(Mandatory = $true)][string]$Label
  )

  $stdout = Join-Path $env:RUNNER_TEMP "$($Label.ToLowerInvariant())-startup-stdout.txt"
  $stderr = Join-Path $env:RUNNER_TEMP "$($Label.ToLowerInvariant())-startup-stderr.txt"
  $process = Start-Process -FilePath $App.FullName -WorkingDirectory $App.DirectoryName `
    -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
  try {
    $deadline = [DateTime]::UtcNow.AddSeconds(12)
    do {
      Start-Sleep -Milliseconds 500
      $process.Refresh()
    } while (-not $process.HasExited -and [DateTime]::UtcNow -lt $deadline)
    if ($process.HasExited) {
      if (Test-Path -LiteralPath $stdout) { Get-Content -LiteralPath $stdout }
      if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr }
      throw "$Label packaged app exited during startup with code $($process.ExitCode)"
    }
  }
  finally {
    if (-not $process.HasExited) {
      $process.Kill($true)
      $process.WaitForExit()
    }
  }
}

$bundleRoot = Join-Path $env:GITHUB_WORKSPACE 'src-tauri\target\release\bundle'
$msiRoot = Join-Path $env:RUNNER_TEMP 'depoaudio-msi-smoke'
if (-not (Test-Path -LiteralPath $msiRoot -PathType Container)) {
  throw 'The MSI extraction is unavailable for packaged native smoke testing'
}

$nsis = Get-ChildItem -LiteralPath (Join-Path $bundleRoot 'nsis') -Filter '*.exe' -File | Select-Object -First 1
if (-not $nsis) { throw 'No packaged NSIS installer was found' }
$sevenZip = Get-Command 7z.exe -ErrorAction Stop
$nsisRoot = Join-Path $env:RUNNER_TEMP 'depoaudio-nsis-audit'
New-Item -ItemType Directory -Force -Path $nsisRoot | Out-Null
& $sevenZip.Source x $nsis.FullName "-o$nsisRoot" -y | Out-Host
if ($LASTEXITCODE -ne 0) { throw '7-Zip could not extract the NSIS installer' }

$nsisApp = Get-ChildItem -LiteralPath $nsisRoot -Recurse -Filter 'depo-audio.exe' -File | Select-Object -First 1
if (-not $nsisApp) {
  $payloadArchives = @(Get-ChildItem -LiteralPath $nsisRoot -Recurse -Filter '*.7z' -File)
  if ($payloadArchives.Count -eq 0) {
    throw 'The extracted NSIS installer contains neither the app nor an embedded payload archive'
  }
  $expandedRoot = Join-Path $nsisRoot 'expanded-payloads'
  New-Item -ItemType Directory -Force -Path $expandedRoot | Out-Null
  $payloadIndex = 0
  foreach ($archive in $payloadArchives) {
    $payloadIndex += 1
    $destination = Join-Path $expandedRoot "payload-$payloadIndex"
    New-Item -ItemType Directory -Force -Path $destination | Out-Null
    & $sevenZip.Source x $archive.FullName "-o$destination" -y | Out-Host
    if ($LASTEXITCODE -ne 0) { throw "7-Zip could not extract NSIS payload $($archive.Name)" }
  }
  $nsisApp = Get-ChildItem -LiteralPath $expandedRoot -Recurse -Filter 'depo-audio.exe' -File | Select-Object -First 1
}
if (-not $nsisApp) { throw 'The extracted NSIS payload does not contain depo-audio.exe' }

node scripts/private-ftr-fixture.mjs --download
if ($LASTEXITCODE -ne 0) { throw 'The private FTR fixture could not be prepared' }
@(
  'FTR_FIXTURE_URL'
  'FTR_FIXTURE_AUTHORIZATION'
  'FTR_FIXTURE_SIZE'
  'FTR_FIXTURE_SHA256'
  'FTR_FIXTURE_EVIDENCE_ID'
) | ForEach-Object { Remove-Item -LiteralPath "Env:$_" -ErrorAction SilentlyContinue }
try {
  Invoke-PackagedNativeSmoke -Root $msiRoot -Label 'MSI'
  Invoke-PackagedNativeSmoke -Root $nsisRoot -Label 'NSIS'
  Invoke-PackagedStartup -App $nsisApp -Label 'NSIS'
}
finally {
  node scripts/private-ftr-fixture.mjs --clean
  Get-ChildItem -LiteralPath $env:RUNNER_TEMP -Filter 'packaged-*-smoke.*' -File -ErrorAction SilentlyContinue |
    Remove-Item -Force -ErrorAction SilentlyContinue
}
