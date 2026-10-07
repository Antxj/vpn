[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$crateRoot = (Resolve-Path $PSScriptRoot).Path.TrimEnd('\')
$userRoot = if ($env:USERPROFILE) {
    $env:USERPROFILE.TrimEnd('\')
} elseif ($env:HOMEDRIVE -and $env:HOMEPATH) {
    ($env:HOMEDRIVE + $env:HOMEPATH).TrimEnd('\')
} else {
    [Environment]::GetFolderPath("UserProfile").TrimEnd('\')
}
$cargoRoot = if ($env:CARGO_HOME) {
    (Resolve-Path $env:CARGO_HOME).Path.TrimEnd('\')
} else {
    (Join-Path $userRoot ".cargo").TrimEnd('\')
}
$targetRoot = (Join-Path $crateRoot "target").TrimEnd('\')

# Cargo uses ASCII unit separator so paths containing spaces remain one rustc argument.
$separator = [char]0x1f
$remapFlags = @(
    "--remap-path-prefix=$userRoot=home"
    "--remap-path-prefix=$cargoRoot=cargo"
    "--remap-path-prefix=$crateRoot=src"
    "--remap-path-prefix=$targetRoot=target"
)
$env:CARGO_ENCODED_RUSTFLAGS = $remapFlags -join $separator

# --- official OpenVPN installer embedded in the executable -----------------
# The MSI is not in the repository: it is downloaded from the official server
# and verified by SHA-256 (and by its Authenticode signature) before going in.
$msiVersao = "2.7.6-I001"
$msiUrl = "https://build.openvpn.net/downloads/releases/OpenVPN-$msiVersao-amd64.msi"
$msiSha256 = "48C96AC092A81C6303F059EACE51C7BF878794E9575B329E82894848E0E529FC"
$msiPath = Join-Path $crateRoot "assets\openvpn.msi"

if (-not (Test-Path $msiPath) -or
    (Get-FileHash -Algorithm SHA256 $msiPath).Hash -ne $msiSha256) {
    Write-Host "Downloading the official OpenVPN installer ($msiVersao)..."
    $ProgressPreference = "SilentlyContinue"
    Invoke-WebRequest -Uri $msiUrl -OutFile $msiPath -UseBasicParsing -TimeoutSec 300
}

$hashBaixado = (Get-FileHash -Algorithm SHA256 $msiPath).Hash
if ($hashBaixado -ne $msiSha256) {
    throw "MSI SHA-256 does not match (expected $msiSha256, got $hashBaixado)."
}
$assinatura = Get-AuthenticodeSignature $msiPath
if ($assinatura.Status -ne "Valid" -or
    $assinatura.SignerCertificate.Subject -notmatch "OpenVPN") {
    throw "Invalid MSI signature or a different publisher: $($assinatura.Status)"
}
Write-Host "OpenVPN MSI verified (SHA-256 and OpenVPN Inc. signature)."

Push-Location $crateRoot
try {
    cargo test --locked
    if ($LASTEXITCODE -ne 0) {
        throw "The tests failed. The release was not built."
    }

    cargo clean --release
    if ($LASTEXITCODE -ne 0) {
        throw "Could not clean old release artifacts."
    }

    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) {
        throw "The release build failed."
    }

    $binary = Join-Path $targetRoot "release\vpn.exe"
    $bytes = [IO.File]::ReadAllBytes($binary)
    $singleByteText = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    $utf16Text = [Text.Encoding]::Unicode.GetString($bytes)
    $forbidden = @($userRoot, $crateRoot, $env:USERNAME, $env:COMPUTERNAME) |
        Where-Object { -not [string]::IsNullOrWhiteSpace($_) } |
        Select-Object -Unique

    foreach ($value in $forbidden) {
        $comparison = [StringComparison]::OrdinalIgnoreCase
        if ($singleByteText.IndexOf($value, $comparison) -ge 0 -or
            $utf16Text.IndexOf($value, $comparison) -ge 0) {
            throw "The executable contains local build information: $value"
        }
    }

    $tamanhoMsi = (Get-Item $msiPath).Length
    if ((Get-Item $binary).Length -lt $tamanhoMsi) {
        throw "The executable is smaller than the MSI: the installer was not embedded."
    }

    $hash = (Get-FileHash -Algorithm SHA256 $binary).Hash
    Write-Host "Release verified: $binary"
    Write-Host "Embedded OpenVPN installer: $msiVersao"
    Write-Host "SHA256: $hash"
} finally {
    Pop-Location
}
