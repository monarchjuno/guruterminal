$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$target = "x86_64-pc-windows-msvc"
$scriptDir = $PSScriptRoot
$appRoot = Split-Path -Parent $scriptDir
$pythonRoot = Join-Path $appRoot "python"
$runtimeRoot = Join-Path $appRoot "src-tauri\resources\pi-runtime"
$workerResource = Join-Path $runtimeRoot "finance-worker"
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ("guruterminal-finance-" + [guid]::NewGuid())
$distRoot = Join-Path $temporaryRoot "dist"

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "Finance worker staging requires 64-bit Windows."
}
if (-not (Test-Path -LiteralPath $runtimeRoot)) {
    throw "Stage Pi before staging the finance worker."
}
if (-not $env:UV_CACHE_DIR) {
    $env:UV_CACHE_DIR = Join-Path ([IO.Path]::GetTempPath()) "guruterminal-uv-cache"
}

New-Item -ItemType Directory -Force -Path $temporaryRoot | Out-Null
try {
    & uv sync --project $pythonRoot --locked --python 3.12
    if ($LASTEXITCODE -ne 0) {
        throw "uv sync failed."
    }
    & uv run --project $pythonRoot --locked python `
        (Join-Path $pythonRoot "build_worker.py") `
        --distpath $distRoot
    if ($LASTEXITCODE -ne 0) {
        throw "Finance worker build failed."
    }

    $builtWorker = Join-Path $distRoot "guruterminal-finance"
    $builtExecutable = Join-Path $builtWorker "guruterminal-finance.exe"
    if (-not (Test-Path -LiteralPath $builtExecutable) -or
        -not (Test-Path -LiteralPath (Join-Path $builtWorker "_internal"))) {
        throw "PyInstaller did not create a complete one-directory finance worker."
    }

    & (Join-Path $scriptDir "sign-windows-binary.ps1") -Path $builtExecutable
    if (Test-Path -LiteralPath $workerResource) {
        Remove-Item -LiteralPath $workerResource -Recurse -Force
    }
    Copy-Item -LiteralPath $builtWorker -Destination $workerResource -Recurse
} finally {
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}

Write-Host "Staged the PyInstaller finance worker for $target."
