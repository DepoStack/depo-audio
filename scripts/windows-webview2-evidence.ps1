[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$OutputPath
)

# DepoAudio's Windows installers do not carry Microsoft's WebView2 runtime or
# its installer. tauri.conf.json selects the download bootstrapper: on a
# machine without WebView2 (Windows 11 includes it), the installer downloads
# Microsoft's bootstrapper from Microsoft at install time and runs it.
#
# This script fails the release unless the bundled configuration selects that
# mode and neither installer carries a WebView2 installer: not as a file in
# the extracted MSI or NSIS payload, and not as a row in the MSI Binary table
# (where Tauri's WiX template puts an embedded or offline installer). It runs
# after scripts/windows-packaged-smoke.ps1 has extracted both installers.

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$tauriCliPackagePath = Join-Path $repoRoot 'node_modules\@tauri-apps\cli\package.json'
if (-not (Test-Path -LiteralPath $tauriCliPackagePath -PathType Leaf)) {
  throw 'The npm-ci-installed Tauri CLI package is unavailable for WebView2 evidence verification'
}
$tauriCliVersion = (Get-Content -LiteralPath $tauriCliPackagePath -Raw | ConvertFrom-Json).version
# Tauri's installer templates decide where a WebView2 installer would sit and
# how the bootstrapper is fetched. Review them again before changing the CLI.
if ($tauriCliVersion -ne '2.12.4') {
  throw "WebView2 evidence must be reviewed for the locked Tauri CLI; expected 2.12.4, found $tauriCliVersion"
}

$tauriConfig = Get-Content -LiteralPath (Join-Path $repoRoot 'src-tauri\tauri.conf.json') -Raw | ConvertFrom-Json
$installMode = $tauriConfig.bundle.windows.webviewInstallMode
if ($installMode.type -ne 'downloadBootstrapper') {
  throw "tauri.conf.json must select the WebView2 download bootstrapper; found '$($installMode.type)'"
}
if ($installMode.silent -ne $true) {
  throw 'tauri.conf.json must run the WebView2 bootstrapper silently'
}
# The release build passes tauri.release.conf.json as an overlay. It must not
# change the install mode.
$releaseConfigPath = Join-Path $repoRoot 'src-tauri\tauri.release.conf.json'
if (Test-Path -LiteralPath $releaseConfigPath -PathType Leaf) {
  $releaseWindows = (Get-Content -LiteralPath $releaseConfigPath -Raw | ConvertFrom-Json).bundle.windows
  if ($releaseWindows -and ($releaseWindows.PSObject.Properties.Name -contains 'webviewInstallMode')) {
    throw 'tauri.release.conf.json must not override the WebView2 install mode'
  }
}

$bundleRoot = Join-Path $repoRoot 'src-tauri\target\release\bundle'
$msi = @(Get-ChildItem -LiteralPath (Join-Path $bundleRoot 'msi') -Filter '*.msi' -File)
$nsis = @(Get-ChildItem -LiteralPath (Join-Path $bundleRoot 'nsis') -Filter '*.exe' -File)
if ($msi.Count -ne 1 -or $nsis.Count -ne 1) {
  throw "Expected exactly one MSI and one NSIS installer; found $($msi.Count) and $($nsis.Count)"
}

$webviewName = '(?i)webview2|edgewebview'
$payloads = [ordered]@{
  msi = Join-Path $env:RUNNER_TEMP 'depoaudio-msi-smoke'
  nsis = Join-Path $env:RUNNER_TEMP 'depoaudio-nsis-audit'
}
$payloadEvidence = [ordered]@{}
foreach ($kind in $payloads.Keys) {
  $root = $payloads[$kind]
  if (-not (Test-Path -LiteralPath $root -PathType Container)) {
    throw "The extracted $kind payload is unavailable: $root"
  }
  $files = @(Get-ChildItem -LiteralPath $root -Recurse -File)
  if ($files.Count -eq 0) { throw "The extracted $kind payload is empty" }
  $found = @($files | Where-Object { $_.Name -match $webviewName })
  if ($found.Count -gt 0) {
    throw "The $kind installer carries a WebView2 installer: $(($found | ForEach-Object Name) -join ', ')"
  }
  $payloadEvidence[$kind] = [ordered]@{ filesScanned = $files.Count; webviewInstallerFiles = 0 }
}

function Invoke-Com($Target, [string]$Member, [string]$Kind, [object[]]$Arguments) {
  return $Target.GetType().InvokeMember($Member, $Kind, $null, $Target, $Arguments)
}

function Get-MsiRows([string]$MsiPath, [string]$Query, [int[]]$StringColumns, [int]$SizeColumn) {
  $installer = New-Object -ComObject WindowsInstaller.Installer
  $database = Invoke-Com $installer 'OpenDatabase' 'InvokeMethod' @($MsiPath, 0)
  $view = Invoke-Com $database 'OpenView' 'InvokeMethod' @($Query)
  [void](Invoke-Com $view 'Execute' 'InvokeMethod' @())
  $rows = [Collections.Generic.List[object]]::new()
  while ($true) {
    $record = Invoke-Com $view 'Fetch' 'InvokeMethod' @()
    if ($null -eq $record) { break }
    $row = [ordered]@{}
    foreach ($column in $StringColumns) {
      $row["column$column"] = [string](Invoke-Com $record 'StringData' 'GetProperty' @($column))
    }
    if ($SizeColumn -gt 0) {
      $row['size'] = [int64](Invoke-Com $record 'DataSize' 'GetProperty' @($SizeColumn))
    }
    $rows.Add([pscustomobject]$row)
  }
  [void](Invoke-Com $view 'Close' 'InvokeMethod' @())
  return $rows
}

$binaryRows = @(Get-MsiRows $msi[0].FullName 'SELECT `Name`, `Data` FROM `Binary`' @(1) 2)
$embedded = @($binaryRows | Where-Object { $_.column1 -match $webviewName })
if ($embedded.Count -gt 0) {
  throw "The MSI Binary table carries a WebView2 installer: $(($embedded | ForEach-Object column1) -join ', ')"
}
$customActions = @(Get-MsiRows $msi[0].FullName 'SELECT `Action`, `Target` FROM `CustomAction`' @(1, 2) 0)
$downloadAction = @($customActions | Where-Object { $_.column2 -match '(?i)go\.microsoft\.com/fwlink' })
# Without this step a Windows 10 machine lacking WebView2 would get no runtime.
if ($downloadAction.Count -eq 0) {
  throw 'The MSI has no custom action that downloads the WebView2 bootstrapper from Microsoft'
}

$evidence = [ordered]@{
  schemaVersion = 2
  tauriCliVersion = $tauriCliVersion
  webviewInstallMode = [ordered]@{ type = $installMode.type; silent = $installMode.silent }
  microsoftBinariesRedistributed = $false
  installers = [ordered]@{
    msi = [ordered]@{ name = $msi[0].Name; size = $msi[0].Length }
    nsis = [ordered]@{ name = $nsis[0].Name; size = $nsis[0].Length }
  }
  extractedPayloads = $payloadEvidence
  msiBinaryTable = @($binaryRows | ForEach-Object { [ordered]@{ name = $_.column1; size = $_.size } })
  # Tauri's WiX template fetches the bootstrapper in this custom action. A
  # reviewer still confirms the download on a clean Windows 10 install.
  msiBootstrapperDownloadActions = @($downloadAction | ForEach-Object column1)
}

$outputParent = Split-Path -Parent $OutputPath
if ($outputParent) { New-Item -ItemType Directory -Force -Path $outputParent | Out-Null }
$evidence | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $OutputPath -Encoding utf8NoBOM
Get-Content -LiteralPath $OutputPath
Write-Host "WebView2 install-mode and payload evidence: $OutputPath"
