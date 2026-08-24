$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$target = "x86_64-pc-windows-msvc"
$scriptDir = $PSScriptRoot
$appRoot = Split-Path -Parent $scriptDir
$repoRoot = (Resolve-Path (Join-Path $appRoot "..\..")).Path
$binaryDir = Join-Path $appRoot "src-tauri\binaries"
$stagedBinary = Join-Path $binaryDir "guruterminal-core-$target.exe"
$cargoManifest = Join-Path $repoRoot "Cargo.toml"
$versionLine = Select-String -LiteralPath $cargoManifest -Pattern '^version = "([^"]+)"' | Select-Object -First 1
if (-not $versionLine) {
    throw "Guru Terminal Core version is missing from Cargo.toml."
}
$expectedVersion = $versionLine.Matches[0].Groups[1].Value

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "Guru Terminal Core staging requires 64-bit Windows."
}

if ($env:GURUTERMINAL_CORE_BIN) {
    $sourceBinary = $env:GURUTERMINAL_CORE_BIN
} else {
    & cargo build `
        --manifest-path $cargoManifest `
        --release `
        --locked `
        --target $target
    if ($LASTEXITCODE -ne 0) {
        throw "Guru Terminal Core compilation failed."
    }
    $sourceBinary = Join-Path $repoRoot "target\$target\release\guruterminal-core.exe"
}

$reportedVersion = (& $sourceBinary --version | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $reportedVersion -ne $expectedVersion) {
    throw "Guru Terminal Core binary version does not match Cargo.toml."
}

New-Item -ItemType Directory -Force -Path $binaryDir | Out-Null
Copy-Item -LiteralPath $sourceBinary -Destination $stagedBinary -Force
& (Join-Path $scriptDir "sign-windows-binary.ps1") -Path $stagedBinary

Write-Host "Staged Guru Terminal Core v$expectedVersion for $target."
