[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$OutputPath,

  [Parameter(Mandatory = $true)]
  [string]$SignatureReportPath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$tauriCliPackagePath = Join-Path $repoRoot 'node_modules\@tauri-apps\cli\package.json'
if (-not (Test-Path -LiteralPath $tauriCliPackagePath -PathType Leaf)) {
  throw 'The npm-ci-installed Tauri CLI package is unavailable for WebView2 evidence verification'
}
$tauriCliPackage = Get-Content -LiteralPath $tauriCliPackagePath -Raw | ConvertFrom-Json
$tauriCliVersion = $tauriCliPackage.version
if ($tauriCliVersion -ne '2.11.4') {
  throw "WebView2 evidence paths must be reviewed for the locked Tauri CLI; expected 2.11.4, found $tauriCliVersion"
}

# tauri-bundler 2.9.4 deletes and recreates target/release/wix/x64, then puts
# the x64 offline installer at wix/x64/x64/<resolved-guid>/<resolved-filename>.
# It separately caches the NSIS input at %LOCALAPPDATA%/tauri/x64 using the same
# moving Microsoft fwlink resolution. These paths come from the exact source
# used by @tauri-apps/cli 2.11.4; this script intentionally fails if they drift.
$wixDownloadRoot = Join-Path $repoRoot 'src-tauri\target\release\wix\x64\x64'
if (-not (Test-Path -LiteralPath $wixDownloadRoot -PathType Container)) {
  throw "Tauri WiX WebView2 download directory is missing: $wixDownloadRoot"
}

$wixCandidates = @(Get-ChildItem -LiteralPath $wixDownloadRoot -Recurse -File)
if ($wixCandidates.Count -ne 1) {
  throw "Expected exactly one Tauri WiX WebView2 build input; found $($wixCandidates.Count)"
}
$wixInput = $wixCandidates[0]
$cacheIdentity = [IO.Path]::GetRelativePath($wixDownloadRoot, $wixInput.FullName)
$identityParts = @($cacheIdentity -split '[\\/]')
$invalidIdentityParts = @($identityParts | Where-Object { $_ -in @('', '.', '..') })
if ($identityParts.Count -ne 2 -or $invalidIdentityParts.Count -gt 0) {
  throw "Unexpected Tauri WebView2 cache identity: $cacheIdentity"
}
if ([IO.Path]::GetExtension($identityParts[1]) -ne '.exe') {
  throw "Tauri WebView2 build input is not an executable: $($identityParts[1])"
}

if (-not $env:LOCALAPPDATA) {
  throw 'LOCALAPPDATA is unavailable; cannot resolve the Tauri NSIS cache'
}
$nsisInputPath = Join-Path (Join-Path $env:LOCALAPPDATA 'tauri\x64') $cacheIdentity
if (-not (Test-Path -LiteralPath $nsisInputPath -PathType Leaf)) {
  throw "NSIS did not reuse the same resolved WebView2 cache identity as WiX: $cacheIdentity"
}
$nsisInput = Get-Item -LiteralPath $nsisInputPath

function Get-VerifiedMicrosoftSignature {
  param([Parameter(Mandatory = $true)][IO.FileInfo]$File)

  $signature = Get-AuthenticodeSignature -LiteralPath $File.FullName
  if ($signature.Status.ToString() -ne 'Valid') {
    throw "WebView2 Authenticode signature is not valid for $($File.Name): $($signature.Status)"
  }
  if (-not $signature.SignerCertificate) {
    throw "WebView2 Authenticode signature has no signer certificate for $($File.Name)"
  }
  $subject = $signature.SignerCertificate.Subject
  if ($subject -notmatch '(?i)(^|,\s*)(CN|O)=Microsoft Corporation(,|$)') {
    throw "WebView2 Authenticode signer is not Microsoft Corporation: $subject"
  }

  [ordered]@{
    status = $signature.Status.ToString()
    statusMessage = $signature.StatusMessage
    signerSubject = $subject
    signerIssuer = $signature.SignerCertificate.Issuer
    signerThumbprint = $signature.SignerCertificate.Thumbprint
    signerNotBefore = $signature.SignerCertificate.NotBefore.ToUniversalTime().ToString('o')
    signerNotAfter = $signature.SignerCertificate.NotAfter.ToUniversalTime().ToString('o')
    timestampSignerSubject = if ($signature.TimeStamperCertificate) { $signature.TimeStamperCertificate.Subject } else { $null }
    timestampSignerThumbprint = if ($signature.TimeStamperCertificate) { $signature.TimeStamperCertificate.Thumbprint } else { $null }
  }
}

$wixHash = (Get-FileHash -LiteralPath $wixInput.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
$nsisHash = (Get-FileHash -LiteralPath $nsisInput.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
if ($wixInput.Length -le 0 -or $nsisInput.Length -le 0) {
  throw 'WebView2 build input is empty'
}
if ($wixInput.Length -ne $nsisInput.Length -or $wixHash -ne $nsisHash) {
  throw 'WiX and NSIS resolved different WebView2 offline-installer bytes'
}

$wixSignature = Get-VerifiedMicrosoftSignature -File $wixInput
$nsisSignature = Get-VerifiedMicrosoftSignature -File $nsisInput
$version = $wixInput.VersionInfo
if (-not $version.FileVersion) {
  throw 'WebView2 offline installer has no file version metadata'
}

$signTool = Get-Command signtool.exe -ErrorAction SilentlyContinue
if (-not $signTool) {
  $windowsKits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $signTool = Get-ChildItem -LiteralPath $windowsKits -Recurse -Filter signtool.exe -File -ErrorAction SilentlyContinue |
    Where-Object FullName -Match '[\\/]x64[\\/]signtool\.exe$' |
    Sort-Object FullName -Descending |
    Select-Object -First 1
}
if (-not $signTool) {
  throw 'signtool.exe is unavailable on the Windows release runner'
}
$signToolPath = if ($signTool -is [System.Management.Automation.ApplicationInfo]) {
  $signTool.Source
} else {
  $signTool.FullName
}
# Current Microsoft WebView2 offline installers carry a publicly trusted
# Microsoft primary signature and an additional internal EdgeBuild signature.
# `/all` fails on that internal self-signed chain even when the primary
# Authenticode signature is valid. Verify the primary signature under the
# Windows Authenticode policy; Get-VerifiedMicrosoftSignature above separately
# requires a valid Microsoft signer and records its certificate identity.
$signatureReport = @(& $signToolPath verify /pa /v $wixInput.FullName 2>&1)
$signToolExitCode = $LASTEXITCODE
$sanitizedSignatureReport = $signatureReport | ForEach-Object {
  $_.ToString().Replace($wixInput.FullName, '<WEBVIEW2_OFFLINE_INSTALLER>')
}
if ($signToolExitCode -ne 0) {
  $sanitizedSignatureReport | ForEach-Object { Write-Host $_ }
  throw 'signtool primary-signature verification failed for the WebView2 offline installer'
}
$signatureReportParent = Split-Path -Parent $SignatureReportPath
if ($signatureReportParent) { New-Item -ItemType Directory -Force -Path $signatureReportParent | Out-Null }
[IO.File]::WriteAllLines($SignatureReportPath, $sanitizedSignatureReport, [Text.UTF8Encoding]::new($false))

$cacheIdentityPortable = $cacheIdentity.Replace('\', '/')
$evidence = [ordered]@{
  schemaVersion = 1
  evidenceScope = 'tauri-build-inputs'
  finalInstallerEmbeddingVerified = $false
  sourceCommit = if ($env:GITHUB_SHA) { $env:GITHUB_SHA } else { $null }
  releaseTag = if ($env:RELEASE_TAG) { $env:RELEASE_TAG } else { $null }
  architecture = 'x64'
  tauriCliVersion = $tauriCliVersion
  tauriBundlerSourceTag = 'tauri-bundler-v2.9.4'
  sourceFwlink = 'https://go.microsoft.com/fwlink/?linkid=2124701'
  resolvedDownloadUrl = "https://msedge.sf.dl.delivery.mp.microsoft.com/filestreamingservice/files/$cacheIdentityPortable"
  resolvedCacheIdentity = $cacheIdentityPortable
  artifact = [ordered]@{
    filename = $wixInput.Name
    bytes = $wixInput.Length
    sha256 = $wixHash
    fileVersion = $version.FileVersion
    productVersion = $version.ProductVersion
    productName = $version.ProductName
    companyName = $version.CompanyName
    originalFilename = $version.OriginalFilename
    signature = $wixSignature
  }
  wixBuildInput = [ordered]@{
    cacheIdentity = $cacheIdentityPortable
    signature = $wixSignature
  }
  nsisBuildInput = [ordered]@{
    cacheIdentity = $cacheIdentityPortable
    signature = $nsisSignature
  }
  byteIdenticalAcrossBundlers = $true
  signatureVerificationReport = [IO.Path]::GetFileName($SignatureReportPath)
  remainingManualGate = 'Extract the final MSI Binary table and NSIS payload, match this SHA-256, and review the exact Microsoft redistribution terms.'
}

$outputParent = Split-Path -Parent $OutputPath
if ($outputParent) { New-Item -ItemType Directory -Force -Path $outputParent | Out-Null }
$evidence | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $OutputPath -Encoding utf8NoBOM
Write-Host "Verified Microsoft-signed, byte-identical WebView2 build inputs: $($wixInput.Name) ($($wixInput.Length) bytes)"
Write-Host "WebView2 build-input evidence: $OutputPath"
