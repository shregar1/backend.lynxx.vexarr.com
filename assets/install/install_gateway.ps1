# Lynxx Gateway installer — Windows.
#
#   irm https://backend.lynxx.vexarr.com/install_gateway.ps1 | iex
#
# Detects 64-bit Windows, asks the Lynxx gateway to stream the binary from
# private GCS, installs into %LOCALAPPDATA%\Programs\lynxx-gateway, and adds
# it to the user PATH (no admin required).

$ErrorActionPreference = 'Stop'

$ApiBase = if ($env:LYNXX_API_BASE) { $env:LYNXX_API_BASE } else { 'https://backend.lynxx.vexarr.com' }
$InstallDir = if ($env:LYNXX_INSTALL_DIR) { $env:LYNXX_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\lynxx-gateway' }
$Force = $env:LYNXX_FORCE -eq '1'
$BinName = 'lynxx-gateway.exe'
$Artifact = 'gateway'

function Write-Banner {
  if (-not $Host.UI.RawUI) { return }
  Write-Host ''
  Write-Host '   _                            ____       _' -ForegroundColor Cyan
  Write-Host '  | |   _   _ _ __ __  ____  __/ ___| __ _| |_ _____      ____ _ _   _' -ForegroundColor Cyan
  Write-Host "  | |  | | | | '_ \ \ \/ /\ \/ / |  _ / _``| __/ _ \ \ /\ / / _``| | | |" -ForegroundColor Cyan
  Write-Host '  | |__| |_| | | | |>  <  >  <| |_| | (_| | ||  __/\ V  V / (_| | |_| |' -ForegroundColor Cyan
  Write-Host '  |_____\__, |_| |_/_/\_\/_/\_\\____|\__,_|\__\___| \_/\_/ \__,_|\__, |' -ForegroundColor Cyan
  Write-Host '        |___/                                                    |___/' -ForegroundColor Cyan
  Write-Host ''
  Write-Host '  Lynxx Gateway' -ForegroundColor DarkGray
  Write-Host ''
}
function Write-Step($msg) { Write-Host "▸ $msg" -ForegroundColor Blue }
function Write-Ok($msg)   { Write-Host "✓ $msg" -ForegroundColor Green }
function Write-Warn($msg) { Write-Host "! $msg" -ForegroundColor Yellow }
function Write-Err($msg)  { Write-Host "✗ $msg" -ForegroundColor Red; exit 1 }
function Write-Dim($msg)  { Write-Host "  $msg" -ForegroundColor DarkGray }

$LynxxReleasePubkey = @'
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAZgjAeMP07kQxUR487qgScW68+r1pbwObAafiYOqc65g=
-----END PUBLIC KEY-----
'@

$LynxxWitnessPubkey = @'
-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAZTPJ1gFfsAmBGGoldLmSN1ni/pKHH4kdNsdneIe7STw=
-----END PUBLIC KEY-----
'@

function Verify-Artifact($binary, $respHeaders) {
  $expectedSha = $respHeaders['X-Content-SHA256']
  if ($expectedSha) {
    if ($expectedSha -is [array]) { $expectedSha = $expectedSha[0] }
    $expectedSha = $expectedSha.ToString().Trim().ToLower()
    $actualSha = (Get-FileHash -Path $binary -Algorithm SHA256).Hash.ToLower()
    if ($actualSha -ne $expectedSha) {
      Remove-Item -Path $binary -Force -ErrorAction SilentlyContinue
      Write-Err "checksum mismatch — expected $expectedSha, got $actualSha.`n  Refusing to install."
    }
    Write-Ok ("checksum verified ({0}…)" -f $expectedSha.Substring(0, 12))
  } else {
    Write-Warn 'no X-Content-SHA256 header — installing without checksum verification.'
  }
  $sigB64 = $respHeaders['X-Content-Signature']
  if (-not $sigB64) {
    Write-Warn 'no X-Content-Signature header — installing without signature verification.'
    return
  }
  if ($sigB64 -is [array]) { $sigB64 = $sigB64[0] }
  $sigB64 = $sigB64.ToString().Trim()
  $opensslCmd = Get-Command openssl -ErrorAction SilentlyContinue
  if (-not $opensslCmd) {
    Write-Warn 'openssl not available — skipping signature verification.'
    return
  }
  $algList = & openssl list -public-key-algorithms 2>$null
  if (-not ($algList -match '(?m)^\s*Name:\s*ED25519')) {
    Write-Warn 'openssl lacks Ed25519 support — skipping signature verification.'
    return
  }
  $pubkeyPath = Join-Path $tmp 'lynxx-pubkey.pem'
  $sigPath    = Join-Path $tmp 'binary.sig'
  Set-Content -Path $pubkeyPath -Value $LynxxReleasePubkey -NoNewline
  [IO.File]::WriteAllBytes($sigPath, [Convert]::FromBase64String($sigB64))
  & openssl pkeyutl -verify -pubin -inkey $pubkeyPath -rawin -in $binary -sigfile $sigPath 2>$null
  if ($LASTEXITCODE -ne 0) {
    Remove-Item -Path $binary -Force -ErrorAction SilentlyContinue
    Write-Err "signature verification FAILED.`n  Refusing to install."
  }
  Write-Ok 'signature verified (Ed25519)'

  Verify-Transparency $expectedSha $sigB64 $pubkeyPath
}

function Verify-Transparency($expectedSha, $expectedSigB64, $pubkeyPath) {
  if (-not $expectedSha) { return }
  $opensslCmd = Get-Command openssl -ErrorAction SilentlyContinue
  if (-not $opensslCmd) {
    Write-Warn 'openssl not available — skipping transparency log verification.'
    return
  }
  $algList = & openssl list -public-key-algorithms 2>$null
  if (-not ($algList -match '(?m)^\s*Name:\s*ED25519')) {
    Write-Warn 'openssl lacks Ed25519 — skipping transparency log verification.'
    return
  }
  $entryFile = Join-Path $tmp 'tlog-entry.json'
  $sigFile   = Join-Path $tmp 'tlog-entry.sig'
  $url = "$ApiBase/v1/install/transparency?sha256=$expectedSha"
  try {
    $resp = Invoke-WebRequest -Uri $url -OutFile $entryFile -UseBasicParsing -PassThru
  } catch {
    Write-Err "binary not in transparency log — refusing to install.`n  Off-the-record signing detected. $_"
  }
  $entrySigB64 = $resp.Headers['X-Transparency-Signature']
  if (-not $entrySigB64) {
    Write-Warn 'no X-Transparency-Signature header — skipping entry verification.'
    return
  }
  if ($entrySigB64 -is [array]) { $entrySigB64 = $entrySigB64[0] }
  $entrySigB64 = $entrySigB64.ToString().Trim()
  [IO.File]::WriteAllBytes($sigFile, [Convert]::FromBase64String($entrySigB64))
  & openssl pkeyutl -verify -pubin -inkey $pubkeyPath -rawin -in $entryFile -sigfile $sigFile 2>$null
  if ($LASTEXITCODE -ne 0) {
    Write-Err 'transparency log entry signature INVALID — refusing to install.'
  }
  $entryText = Get-Content -Path $entryFile -Raw
  $entrySha = ($entryText | Select-String -Pattern '"sha256":"([^"]+)"').Matches[0].Groups[1].Value
  $entrySig = ($entryText | Select-String -Pattern '"sig":"([^"]+)"').Matches[0].Groups[1].Value
  if ($entrySha -ne $expectedSha) {
    Write-Err 'transparency log sha256 mismatch — refusing to install.'
  }
  if ($entrySig -ne $expectedSigB64) {
    Write-Err 'transparency log binary-signature mismatch — refusing to install.'
  }
  Write-Ok 'transparency log verified'

  Verify-WitnessRoot $expectedSha
}

function Verify-WitnessRoot($expectedSha) {
  if (-not $expectedSha) { return }
  $rootFile = Join-Path $tmp 'witness-root.json'
  $sigFile  = Join-Path $tmp 'witness-root.sig'
  $witnessPem = Join-Path $tmp 'witness-pubkey.pem'
  Set-Content -Path $witnessPem -Value $LynxxWitnessPubkey -NoNewline
  try {
    $resp = Invoke-WebRequest -Uri "$ApiBase/v1/install/transparency-root" -OutFile $rootFile -UseBasicParsing -PassThru
  } catch {
    Write-Warn "no witness-signed root available — installing WITHOUT witness coverage. $_"
    if ($env:LYNXX_REQUIRE_WITNESS -eq '1') { Write-Err 'LYNXX_REQUIRE_WITNESS=1 and no witness root.' }
    return
  }
  $witnessSigB64 = $resp.Headers['X-Witness-Signature']
  if (-not $witnessSigB64) { Write-Warn 'X-Witness-Signature missing — skipping.'; return }
  if ($witnessSigB64 -is [array]) { $witnessSigB64 = $witnessSigB64[0] }
  [IO.File]::WriteAllBytes($sigFile, [Convert]::FromBase64String($witnessSigB64.ToString().Trim()))
  & openssl pkeyutl -verify -pubin -inkey $witnessPem -rawin -in $rootFile -sigfile $sigFile 2>$null
  if ($LASTEXITCODE -ne 0) { Write-Err 'witness root signature INVALID — refusing to install.' }
  $rootText = Get-Content -Path $rootFile -Raw
  if ($rootText -notmatch "`"$expectedSha`"") {
    Write-Err 'sha256 not in witness-signed root — off-the-record signing detected. Refusing.'
  }
  Write-Ok 'witness coverage verified (Ed25519)'
}

function Get-Arch {
  if ([Environment]::Is64BitOperatingSystem) {
    $proc = (Get-CimInstance Win32_Processor).Architecture
    # 9 = x64, 12 = arm64
    if ($proc -eq 12) { return 'arm64' }
    return 'amd64'
  } else {
    Write-Err 'Lynxx Gateway requires 64-bit Windows.'
  }
}

Write-Banner
$Arch = Get-Arch

$existing = Get-Command lynxx-gateway -ErrorAction SilentlyContinue
if ($existing -and -not $Force) {
  $version = & $existing.Source --version 2>$null
  Write-Warn "lynxx-gateway already installed: $($existing.Source) ($version)"
  $answer = Read-Host '  Reinstall? [y/N]'
  if ($answer -notmatch '^(y|yes)$') {
    Write-Dim '  Skipping. Set $env:LYNXX_FORCE=1 to reinstall non-interactively.'
    return
  }
}

Write-Step "platform   windows/$Arch"
Write-Step "install to $InstallDir\$BinName"

$url = "$ApiBase/v1/install/binary?artifact=$Artifact&os=windows&arch=$Arch"
$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ([System.Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
$tmpFile = Join-Path $tmp $BinName

Write-Step 'downloading...'
try {
  $resp = Invoke-WebRequest -Uri $url -OutFile $tmpFile -UseBasicParsing -PassThru
} catch {
  Write-Err "download failed: $url`n  $_"
}

Verify-Artifact $tmpFile $resp.Headers

Write-Step 'installing...'
New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
Move-Item -Path $tmpFile -Destination (Join-Path $InstallDir $BinName) -Force
Remove-Item -Path $tmp -Recurse -Force -ErrorAction SilentlyContinue

$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$pathParts = $userPath -split ';' | Where-Object { $_ }
if ($pathParts -notcontains $InstallDir) {
  $newPath = ($pathParts + $InstallDir) -join ';'
  [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
  Write-Warn 'Added install dir to your user PATH.'
  Write-Dim '  Open a new terminal for it to take effect.'
}

Write-Host ''
Write-Ok "Installed lynxx-gateway → $InstallDir\$BinName"
Write-Host ''
Write-Host 'Next steps:' -ForegroundColor White
Write-Host '  $ lynxx-gateway --help    ' -NoNewline; Write-Host '# all commands' -ForegroundColor DarkGray
Write-Host ''
