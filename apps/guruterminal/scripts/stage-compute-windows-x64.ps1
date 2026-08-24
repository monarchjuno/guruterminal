$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$target = "x86_64-pc-windows-msvc"
$scriptDir = $PSScriptRoot
$appRoot = Split-Path -Parent $scriptDir
$computeRoot = Join-Path $appRoot "compute"
$tauriRoot = Join-Path $appRoot "src-tauri"
$runtimeRoot = Join-Path $tauriRoot "resources\pi-runtime"
$workerResource = Join-Path $runtimeRoot "compute-worker"
$manifestPath = Join-Path $computeRoot "runtime-manifest.json"
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$denoVersion = $manifest.deno.version
$pyodideVersion = $manifest.pyodide.version
$archiveSpec = $manifest.deno.archives.$target
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ("guruterminal-compute-" + [guid]::NewGuid())
$newWorker = Join-Path $runtimeRoot (".compute-worker-" + [guid]::NewGuid())

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "Compute worker staging requires 64-bit Windows."
}
if (-not (Test-Path -LiteralPath $runtimeRoot)) {
    throw "Stage Pi before staging the compute worker."
}

& npm ci --prefix $computeRoot --ignore-scripts
if ($LASTEXITCODE -ne 0) { throw "Compute runtime npm install failed." }
$installedPyodide = (Get-Content -LiteralPath (Join-Path $computeRoot "node_modules\pyodide\package.json") -Raw | ConvertFrom-Json).version
if ($installedPyodide -ne $pyodideVersion) {
    throw "Installed Pyodide does not match the compute runtime manifest."
}

New-Item -ItemType Directory -Force -Path $temporaryRoot, $newWorker | Out-Null
try {
    $archive = Join-Path $temporaryRoot $archiveSpec.file
    Invoke-WebRequest `
        -Uri "https://github.com/denoland/deno/releases/download/v$denoVersion/$($archiveSpec.file)" `
        -OutFile $archive
    $archiveDigest = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    if ($archiveDigest -ne $archiveSpec.sha256) {
        throw "Deno archive checksum mismatch."
    }
    $extracted = Join-Path $temporaryRoot "deno"
    Expand-Archive -LiteralPath $archive -DestinationPath $extracted
    $denoBinary = Join-Path $extracted "deno.exe"
    if (-not (Test-Path -LiteralPath $denoBinary)) {
        throw "Deno executable is missing after extraction."
    }
    $reportedVersion = ((& $denoBinary --version | Select-Object -First 1) -split ' ')[1]
    if ($reportedVersion -ne $denoVersion) {
        throw "Deno executable version does not match the pinned version."
    }

    $runtimeBinary = Join-Path $newWorker "guruterminal-compute.exe"
    Copy-Item -LiteralPath $denoBinary -Destination $runtimeBinary
    Copy-Item -LiteralPath (Join-Path $computeRoot "bootstrap.mjs"), (Join-Path $computeRoot "javascript-host.mjs"), (Join-Path $computeRoot "contract.mjs"), $manifestPath -Destination $newWorker
    $pyodideTarget = Join-Path $newWorker "pyodide"
    New-Item -ItemType Directory -Force -Path $pyodideTarget | Out-Null
    foreach ($asset in @("pyodide.asm.mjs", "pyodide.asm.wasm", "pyodide.mjs", "pyodide-lock.json", "python_stdlib.zip")) {
        Copy-Item -LiteralPath (Join-Path $computeRoot "node_modules\pyodide\$asset") -Destination (Join-Path $pyodideTarget $asset)
    }
    foreach ($package in $manifest.pyodide.packages) {
        $targetPath = Join-Path $pyodideTarget $package.file
        Invoke-WebRequest -Uri "https://cdn.jsdelivr.net/pyodide/v$pyodideVersion/full/$($package.file)" -OutFile $targetPath
        $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath $targetPath).Hash.ToLowerInvariant()
        if ($digest -ne $package.sha256) {
            throw "Pyodide package checksum mismatch: $($package.file)"
        }
    }

    Set-Content -LiteralPath (Join-Path $newWorker ".deno-version") -Value $denoVersion -Encoding ascii
    Set-Content -LiteralPath (Join-Path $newWorker ".deno-archive.sha256") -Value $archiveSpec.sha256 -Encoding ascii
    Set-Content -LiteralPath (Join-Path $newWorker ".pyodide-version") -Value $pyodideVersion -Encoding ascii
    Set-Content -LiteralPath (Join-Path $newWorker ".compute-manifest.sha256") -Value ((Get-FileHash -Algorithm SHA256 -LiteralPath $manifestPath).Hash.ToLowerInvariant()) -Encoding ascii
    & (Join-Path $scriptDir "sign-windows-binary.ps1") -Path $runtimeBinary
    Set-Content -LiteralPath (Join-Path $newWorker ".compute-executable.sha256") -Value ((Get-FileHash -Algorithm SHA256 -LiteralPath $runtimeBinary).Hash.ToLowerInvariant()) -Encoding ascii

    if (Test-Path -LiteralPath $workerResource) {
        Remove-Item -LiteralPath $workerResource -Recurse -Force
    }
    Move-Item -LiteralPath $newWorker -Destination $workerResource
    $newWorker = $null
} finally {
    if ($newWorker -and (Test-Path -LiteralPath $newWorker)) {
        Remove-Item -LiteralPath $newWorker -Recurse -Force
    }
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}

Write-Host "Staged the Deno/Pyodide compute worker for $target."
