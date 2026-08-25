$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-X64Pe([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $reader = [IO.BinaryReader]::new($stream)
    try {
        if ($reader.ReadUInt16() -ne 0x5A4D) {
            throw "$Path is not a PE executable."
        }
        $stream.Position = 0x3C
        $peOffset = $reader.ReadUInt32()
        $stream.Position = $peOffset
        if ($reader.ReadUInt32() -ne 0x00004550) {
            throw "$Path has an invalid PE signature."
        }
        if ($reader.ReadUInt16() -ne 0x8664) {
            throw "$Path is not an x86_64 PE executable."
        }
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
}

$target = "x86_64-pc-windows-msvc"
$piVersion = "0.84.2"
$archiveSha256 = "741fc1ae1afecb573ac2888e011188ff446b3940f4aabe1583f60bf55be8a3d0"
$denoVersion = "2.9.5"
$denoArchiveSha256 = "171efab55ac6b9881fd53ee4c20f8bf3bb1340ffc618483746909014db12216a"
$pyodideVersion = "314.0.3"
$scriptDir = $PSScriptRoot
$appRoot = Split-Path -Parent $scriptDir
$repoRoot = (Resolve-Path (Join-Path $appRoot "..\..")).Path
$tauriRoot = Join-Path $appRoot "src-tauri"
$pythonBinary = Join-Path $appRoot "python\.venv\Scripts\python.exe"
$runtimeDir = Join-Path $tauriRoot "resources\pi-runtime"
$piBinary = Join-Path $runtimeDir "guruterminal-pi.exe"
$coreBinary = Join-Path $tauriRoot "binaries\guruterminal-core-$target.exe"
$financeBinary = Join-Path $runtimeDir "finance-worker\guruterminal-finance.exe"
$computeRuntime = Join-Path $runtimeDir "compute-worker"
$computeBinary = Join-Path $computeRuntime "guruterminal-compute.exe"
$openbbRuntime = Join-Path $runtimeDir "openbb"
$openbbBinary = Join-Path $openbbRuntime "guruterminal-openbb.exe"
$versionLine = Select-String -LiteralPath (Join-Path $repoRoot "Cargo.toml") -Pattern '^version = "([^"]+)"' | Select-Object -First 1
if (-not $versionLine) {
    throw "Guru Terminal Core version is missing from Cargo.toml."
}
$coreVersion = $versionLine.Matches[0].Groups[1].Value

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "Package prerequisite checks require 64-bit Windows."
}
if (-not (Test-Path -LiteralPath $pythonBinary) -or
    (& $pythonBinary -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")') -ne "3.12") {
    throw "The pinned Python 3.12 staging environment is missing: $pythonBinary"
}

$required = @(
    $piBinary,
    $coreBinary,
    $financeBinary,
    $computeBinary,
    $openbbBinary,
    (Join-Path $runtimeDir "package.json"),
    (Join-Path $runtimeDir ".pi-version"),
    (Join-Path $runtimeDir ".pi-archive.sha256"),
    (Join-Path $runtimeDir ".pi-executable.sha256"),
    (Join-Path $runtimeDir "finance-worker\_internal"),
    (Join-Path $computeRuntime "bootstrap.mjs"),
    (Join-Path $computeRuntime "javascript-host.mjs"),
    (Join-Path $computeRuntime "contract.mjs"),
    (Join-Path $computeRuntime "runtime-manifest.json"),
    (Join-Path $computeRuntime "pyodide\pyodide.asm.wasm"),
    (Join-Path $computeRuntime ".deno-version"),
    (Join-Path $computeRuntime ".deno-archive.sha256"),
    (Join-Path $computeRuntime ".pyodide-version"),
    (Join-Path $computeRuntime ".compute-manifest.sha256"),
    (Join-Path $computeRuntime ".compute-executable.sha256"),
    (Join-Path $openbbRuntime "_internal"),
    (Join-Path $openbbRuntime "_internal\guruterminal_openbb\runtime-manifest.json"),
    (Join-Path $openbbRuntime "runtime-manifest.json"),
    (Join-Path $openbbRuntime "THIRD_PARTY_LICENSES\python-distributions.json"),
    (Join-Path $openbbRuntime "uv.lock"),
    (Join-Path $appRoot "agent\guruterminal-extension.mjs"),
    (Join-Path $appRoot "agent\broker-client.mjs"),
    (Join-Path $appRoot "agent\workbench-tools.mjs"),
    (Join-Path $appRoot "agent\model-run-controls.mjs"),
    (Join-Path $appRoot "agent\guruterminal-native-search.mjs"),
    (Join-Path $appRoot "agent\native-search\common.mjs"),
    (Join-Path $appRoot "agent\native-search\codex.mjs"),
    (Join-Path $appRoot "agent\native-search\anthropic.mjs"),
    (Join-Path $appRoot "agent\native-search\xai.mjs"),
    (Join-Path $appRoot "agent\guruterminal-provider-extension.mjs"),
    (Join-Path $appRoot "agent\SYSTEM.md"),
    (Join-Path $appRoot "agent\skills\research\SKILL.md"),
    (Join-Path $appRoot "agent\skills\wiki\SKILL.md"),
    (Join-Path $appRoot "agent\skills\lens\SKILL.md"),
    (Join-Path $appRoot "THIRD_PARTY_NOTICES.md"),
    (Join-Path $repoRoot "LICENSE"),
    (Join-Path $repoRoot "NOTICE")
)
foreach ($path in $required) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Required staged asset is missing: $path"
    }
}
$thirdPartyNotices = Get-Content -LiteralPath (Join-Path $appRoot "THIRD_PARTY_NOTICES.md") -Raw
foreach ($fragment in @("Pi coding agent", "2025 Mario Zechner", "Deno 2.9.5", "Pyodide 314.0.3", "MIT License")) {
    if (-not $thirdPartyNotices.Contains($fragment)) {
        throw "Pi license notice is incomplete: $fragment"
    }
}
$obsoleteRuntimePaths = @(
    (Join-Path $runtimeDir "pi.exe"),
    (Join-Path $runtimeDir "openbb-runtime"),
    (Join-Path $tauriRoot "binaries\guruterminal-pi-$target.exe")
)
foreach ($path in $obsoleteRuntimePaths) {
    if (Test-Path -LiteralPath $path) {
        throw "Obsolete runtime packaging path is present: $path"
    }
}
$localBuildMetadata = @(
    Get-ChildItem -LiteralPath $runtimeDir -Filter "direct_url.json" -Recurse -File |
        Where-Object { $_.Directory.Name.EndsWith(".dist-info", [StringComparison]::Ordinal) }
)
if ($localBuildMetadata.Count -ne 0) {
    throw "Staged runtime contains local build-path metadata."
}
$reparsePoints = @(
    Get-ChildItem -LiteralPath $runtimeDir -Recurse -Force |
        Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 }
)
if ($reparsePoints.Count -ne 0) {
    throw "Staged runtime contains a reparse point."
}

if ((Get-Content -LiteralPath (Join-Path $runtimeDir ".pi-version") -Raw).Trim() -ne $piVersion) {
    throw "Staged Pi version marker is invalid."
}
if ((Get-Content -LiteralPath (Join-Path $runtimeDir ".pi-archive.sha256") -Raw).Trim() -ne $archiveSha256) {
    throw "Staged Pi archive digest marker is invalid."
}
$packageVersion = (Get-Content -LiteralPath (Join-Path $runtimeDir "package.json") -Raw | ConvertFrom-Json).version
if ($packageVersion -ne $piVersion) {
    throw "Staged Pi package metadata has the wrong version."
}
$runtimePiSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $piBinary).Hash.ToLowerInvariant()
$pinnedPiSha256 = (Get-Content -LiteralPath (Join-Path $runtimeDir ".pi-executable.sha256") -Raw).Trim()
if ($runtimePiSha256 -ne $pinnedPiSha256) {
    throw "Pi resource binary differs from its verified digest."
}
if ((Get-Content -LiteralPath (Join-Path $computeRuntime ".deno-version") -Raw).Trim() -ne $denoVersion -or
    (Get-Content -LiteralPath (Join-Path $computeRuntime ".deno-archive.sha256") -Raw).Trim() -ne $denoArchiveSha256 -or
    (Get-Content -LiteralPath (Join-Path $computeRuntime ".pyodide-version") -Raw).Trim() -ne $pyodideVersion) {
    throw "Staged compute runtime identity is invalid."
}
$runtimeComputeSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $computeBinary).Hash.ToLowerInvariant()
$pinnedComputeSha256 = (Get-Content -LiteralPath (Join-Path $computeRuntime ".compute-executable.sha256") -Raw).Trim()
if ($runtimeComputeSha256 -ne $pinnedComputeSha256) {
    throw "Compute resource binary differs from its verified digest."
}
$runtimeComputeManifestSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $computeRuntime "runtime-manifest.json")).Hash.ToLowerInvariant()
$pinnedComputeManifestSha256 = (Get-Content -LiteralPath (Join-Path $computeRuntime ".compute-manifest.sha256") -Raw).Trim()
if ($runtimeComputeManifestSha256 -ne $pinnedComputeManifestSha256) {
    throw "Compute runtime manifest differs from its verified digest."
}

$openbbNativeFiles = @(
    Get-ChildItem -LiteralPath $openbbRuntime -Recurse -File -Force |
        Where-Object { $_.Extension -in @(".exe", ".dll", ".pyd") } |
        ForEach-Object { $_.FullName }
)
if ($openbbNativeFiles.Count -eq 0) {
    throw "OpenBB runtime contains no native Windows files."
}
$nativeBinaries = @($piBinary, $coreBinary, $financeBinary, $computeBinary)
$nativeBinaries += $openbbNativeFiles
foreach ($binary in $nativeBinaries) {
    Assert-X64Pe $binary
}

& node --check (Join-Path $appRoot "agent\guruterminal-extension.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent extension syntax check failed." }
& node --check (Join-Path $appRoot "agent\broker-client.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent broker client syntax check failed." }
& node --check (Join-Path $appRoot "agent\workbench-tools.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent workbench tools syntax check failed." }
& node --check (Join-Path $appRoot "agent\model-run-controls.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent model run controls syntax check failed." }
& node --check (Join-Path $appRoot "agent\guruterminal-native-search.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent native search syntax check failed." }
& node --check (Join-Path $appRoot "agent\native-search\common.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent native search common syntax check failed." }
& node --check (Join-Path $appRoot "agent\native-search\codex.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent Codex native search syntax check failed." }
& node --check (Join-Path $appRoot "agent\native-search\anthropic.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent Anthropic native search syntax check failed." }
& node --check (Join-Path $appRoot "agent\native-search\xai.mjs")
if ($LASTEXITCODE -ne 0) { throw "Agent xAI native search syntax check failed." }
& node --check (Join-Path $appRoot "agent\guruterminal-provider-extension.mjs")
if ($LASTEXITCODE -ne 0) { throw "Provider extension syntax check failed." }
$agentTests = @(
    Get-ChildItem -LiteralPath (Join-Path $appRoot "agent") -Filter "*.test.mjs" -File |
        ForEach-Object { $_.FullName }
)
if ($agentTests.Count -eq 0) { throw "Agent extension tests are missing." }
& node --test @agentTests
if ($LASTEXITCODE -ne 0) { throw "Agent extension tests failed." }

Get-Content -LiteralPath (Join-Path $tauriRoot "tauri.conf.json") -Raw | ConvertFrom-Json | Out-Null
Get-Content -LiteralPath (Join-Path $tauriRoot "tauri.release.conf.json") -Raw | ConvertFrom-Json | Out-Null
Get-Content -LiteralPath (Join-Path $tauriRoot "tauri.package-smoke.conf.json") -Raw | ConvertFrom-Json | Out-Null

& $pythonBinary (Join-Path $scriptDir "check-sidecars.py") `
    --pi $piBinary `
    --pi-version $piVersion `
    --pi-runtime $runtimeDir `
    --provider-extension (Join-Path $appRoot "agent\guruterminal-provider-extension.mjs") `
    --core $coreBinary `
    --core-version $coreVersion `
    --finance $financeBinary `
    --compute $computeBinary `
    --compute-runtime $computeRuntime `
    --deno-version $denoVersion `
    --pyodide-version $pyodideVersion `
    --openbb $openbbBinary `
    --openbb-runtime $openbbRuntime
if ($LASTEXITCODE -ne 0) {
    throw "Sidecar smoke checks failed."
}

if ($env:GURUTERMINAL_REQUIRE_DISTRIBUTION_SIGNING -eq "1") {
    foreach ($binary in $nativeBinaries) {
        $signature = Get-AuthenticodeSignature -LiteralPath $binary
        if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
            throw "Authenticode verification failed for $binary`: $($signature.StatusMessage)"
        }
    }
} else {
    Write-Host "Unsigned staging validated. Distribution signing was not asserted."
}

Write-Host "Guru Terminal Windows x64 package prerequisites passed."
