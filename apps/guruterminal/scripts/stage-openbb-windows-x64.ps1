$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$target = "x86_64-pc-windows-msvc"
$scriptDir = $PSScriptRoot
$appRoot = Split-Path -Parent $scriptDir
$openbbRoot = Join-Path $appRoot "openbb"
$runtimeRoot = Join-Path $appRoot "src-tauri\resources\pi-runtime"
$sidecarResource = Join-Path $runtimeRoot "openbb"
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ("guruterminal-openbb-" + [guid]::NewGuid())
$distRoot = Join-Path $temporaryRoot "dist"

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "OpenBB staging requires 64-bit Windows."
}
if (-not (Test-Path -LiteralPath $runtimeRoot)) {
    throw "Stage Pi before staging OpenBB."
}
if (-not $env:UV_CACHE_DIR) {
    $env:UV_CACHE_DIR = Join-Path ([IO.Path]::GetTempPath()) "guruterminal-uv-cache"
}

New-Item -ItemType Directory -Force -Path $temporaryRoot | Out-Null
try {
    & uv sync --project $openbbRoot --locked --python 3.12
    if ($LASTEXITCODE -ne 0) {
        throw "OpenBB uv sync failed."
    }
    & uv run --project $openbbRoot --locked python `
        (Join-Path $openbbRoot "build_sidecar.py") `
        --distpath $distRoot
    if ($LASTEXITCODE -ne 0) {
        throw "OpenBB runtime build failed."
    }

    $builtRuntime = Join-Path $distRoot "guruterminal-openbb"
    $builtExecutable = Join-Path $builtRuntime "guruterminal-openbb.exe"
    if (-not (Test-Path -LiteralPath $builtExecutable) -or
        -not (Test-Path -LiteralPath (Join-Path $builtRuntime "_internal")) -or
        -not (Test-Path -LiteralPath (Join-Path $builtRuntime "_internal\random_user_agent\data\user_agents.zip") -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $builtRuntime "THIRD_PARTY_LICENSES\python-distributions.json") -PathType Leaf)) {
        throw "PyInstaller did not create a complete OpenBB runtime."
    }

    $peFiles = @(
        Get-ChildItem -LiteralPath $builtRuntime -Recurse -File |
            Where-Object {
                $_.Extension.ToLowerInvariant() -in ".exe", ".dll", ".pyd"
            } |
            Sort-Object FullName
    )
    if ($peFiles.Count -eq 0) {
        throw "OpenBB runtime contains no Windows PE files to sign."
    }
    foreach ($peFile in $peFiles) {
        & (Join-Path $scriptDir "sign-windows-binary.ps1") -Path $peFile.FullName
    }
    $publicManifestPath = Join-Path $builtRuntime "runtime-manifest.json"
    $publicManifest = Get-Content -LiteralPath $publicManifestPath -Raw |
        ConvertFrom-Json
    $publicManifest.executable = "guruterminal-openbb.exe"
    $publicManifest | ConvertTo-Json -Depth 20 |
        Set-Content -LiteralPath $publicManifestPath -Encoding utf8NoBOM
    if (Test-Path -LiteralPath $sidecarResource) {
        Remove-Item -LiteralPath $sidecarResource -Recurse -Force
    }
    Copy-Item -LiteralPath $builtRuntime -Destination $sidecarResource -Recurse
} finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}

Write-Host "Staged the OpenBB runtime for $target."
