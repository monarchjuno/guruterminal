param(
    [Parameter(Mandatory = $true)]
    [string]$Installer,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedVersion,

    [switch]$RequireDistributionSigning
)

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

function Assert-Authenticode([string]$Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
        throw "Authenticode verification failed for $Path`: $($signature.StatusMessage)"
    }
}

function Invoke-CheckedProcess(
    [string]$Path,
    [string[]]$Arguments,
    [int]$TimeoutSeconds = 300
) {
    if ($TimeoutSeconds -lt 1) {
        throw "Process timeout must be positive."
    }
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Path
    $startInfo.UseShellExecute = $false
    foreach ($argument in $Arguments) {
        $startInfo.ArgumentList.Add($argument)
    }
    $process = [Diagnostics.Process]::Start($startInfo)
    if ($null -eq $process) {
        throw "Failed to start process: $Path"
    }
    try {
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            try {
                $process.Kill($true)
            } catch [InvalidOperationException] {
                # The process exited between the timeout and the kill attempt.
            }
            if (-not $process.WaitForExit(10000)) {
                throw "Timed-out process tree could not be stopped: $Path"
            }
            throw "Process exceeded the $TimeoutSeconds-second limit: $Path"
        }
        if ($process.ExitCode -ne 0) {
            throw "Process exited with code $($process.ExitCode): $Path"
        }
    } finally {
        $process.Dispose()
    }
}

function Assert-SameFile([string]$Expected, [string]$Actual) {
    if (-not (Test-Path -LiteralPath $Expected -PathType Leaf) -or
        -not (Test-Path -LiteralPath $Actual -PathType Leaf)) {
        throw "File comparison input is missing: $Expected / $Actual"
    }
    $expectedHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Expected).Hash
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $Actual).Hash
    if ($expectedHash -ne $actualHash) {
        throw "Packaged file differs from its source: $Actual"
    }
}

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "Final NSIS package inspection requires 64-bit Windows."
}
if ($ExpectedVersion -notmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.[1-9][0-9]*)?$') {
    throw "Expected version must be canonical X.Y.Z or X.Y.Z-rc.N."
}

$scriptDir = $PSScriptRoot
$appRoot = Split-Path -Parent $scriptDir
$repoRoot = (Resolve-Path (Join-Path $appRoot "..\..")).Path
$tauriConfigPath = Join-Path $appRoot "src-tauri\tauri.conf.json"
$tauriConfig = Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json
$productName = [string]$tauriConfig.productName
$publisher = [string]$tauriConfig.bundle.publisher
$bundleIdentifier = [string]$tauriConfig.identifier
if ($productName -ne "Guru Terminal" -or
    $publisher -ne "Guru Terminal" -or
    $bundleIdentifier -ne "com.monarchjuno.guruterminal" -or
    $tauriConfig.bundle.windows.nsis.installMode -ne "currentUser" -or
    $tauriConfig.version -ne $ExpectedVersion) {
    throw "Tauri Windows package identity does not match the expected product."
}

$resolvedInstaller = (Resolve-Path -LiteralPath $Installer).Path
$installerItem = Get-Item -LiteralPath $resolvedInstaller -Force
if ($installerItem.PSIsContainer -or
    ($installerItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Installer must be a regular file."
}
$installerVersion = $installerItem.VersionInfo
if ($installerVersion.ProductName -ne $productName -or
    $installerVersion.FileDescription -ne $productName -or
    $installerVersion.ProductVersion -ne $ExpectedVersion) {
    throw "NSIS installer version resources do not match Guru Terminal $ExpectedVersion."
}
if ($RequireDistributionSigning) {
    Assert-Authenticode $resolvedInstaller
}

$pythonBinary = (Get-Command python -ErrorAction Stop).Source
if ((& $pythonBinary -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")') -ne "3.12") {
    throw "Python 3.12 is required for installed sidecar smoke checks."
}

$uninstallRegistryPath = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\$productName"
$publisherRegistryPath = "HKCU:\Software\$publisher"
$productRegistryPath = "HKCU:\Software\$publisher\$productName"
if ((Test-Path -LiteralPath $uninstallRegistryPath) -or
    (Test-Path -LiteralPath $productRegistryPath)) {
    throw "Refusing to replace an existing Guru Terminal installation."
}
$publisherRegistryExisted = Test-Path -LiteralPath $publisherRegistryPath

$temporaryParent = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$temporaryParent = (Resolve-Path -LiteralPath $temporaryParent).Path
$installRoot = Join-Path $temporaryParent ("guruterminal-installed-" + [guid]::NewGuid().ToString("N"))
if (Test-Path -LiteralPath $installRoot) {
    throw "Generated install root already exists."
}
if ($installRoot -match '\s') {
    throw "NSIS /D qualification path must not contain whitespace: $installRoot"
}

$installStarted = $false
$primaryError = $null
$cleanupError = $null
try {
    $installStarted = $true
    Invoke-CheckedProcess $resolvedInstaller @("/S", "/NS", "/D=$installRoot")

    $mainBinary = Join-Path $installRoot "guruterminal.exe"
    $coreBinary = Join-Path $installRoot "guruterminal-core.exe"
    $runtimeDir = Join-Path $installRoot "pi-runtime"
    $piBinary = Join-Path $runtimeDir "guruterminal-pi.exe"
    $financeBinary = Join-Path $runtimeDir "finance-worker\guruterminal-finance.exe"
    $computeRuntime = Join-Path $runtimeDir "compute-worker"
    $computeBinary = Join-Path $computeRuntime "guruterminal-compute.exe"
    $openbbRuntime = Join-Path $runtimeDir "openbb"
    $openbbBinary = Join-Path $openbbRuntime "guruterminal-openbb.exe"
    $agentDir = Join-Path $installRoot "guruterminal-agent"
    $uninstaller = Join-Path $installRoot "uninstall.exe"

    $requiredFiles = @(
        $mainBinary,
        $coreBinary,
        $piBinary,
        $financeBinary,
        $computeBinary,
        $openbbBinary,
        (Join-Path $runtimeDir "package.json"),
        (Join-Path $runtimeDir ".pi-version"),
        (Join-Path $runtimeDir ".pi-archive.sha256"),
        (Join-Path $runtimeDir ".pi-executable.sha256"),
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
        (Join-Path $openbbRuntime "_internal\guruterminal_openbb\runtime-manifest.json"),
        (Join-Path $openbbRuntime "runtime-manifest.json"),
        (Join-Path $openbbRuntime "THIRD_PARTY_LICENSES\python-distributions.json"),
        (Join-Path $openbbRuntime "uv.lock"),
        (Join-Path $agentDir "SYSTEM.md"),
        (Join-Path $agentDir "guruterminal-extension.mjs"),
        (Join-Path $agentDir "broker-client.mjs"),
        (Join-Path $agentDir "workbench-tools.mjs"),
        (Join-Path $agentDir "model-run-controls.mjs"),
        (Join-Path $agentDir "guruterminal-native-search.mjs"),
        (Join-Path $agentDir "native-search\common.mjs"),
        (Join-Path $agentDir "native-search\codex.mjs"),
        (Join-Path $agentDir "native-search\anthropic.mjs"),
        (Join-Path $agentDir "native-search\xai.mjs"),
        (Join-Path $agentDir "guruterminal-provider-extension.mjs"),
        (Join-Path $agentDir "skills\research\SKILL.md"),
        (Join-Path $agentDir "skills\wiki\SKILL.md"),
        (Join-Path $agentDir "skills\lens\SKILL.md"),
        (Join-Path $installRoot "LICENSE"),
        (Join-Path $installRoot "NOTICE"),
        (Join-Path $installRoot "THIRD_PARTY_NOTICES.md"),
        $uninstaller
    )
    foreach ($path in $requiredFiles) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf) -or
            (Get-Item -LiteralPath $path).Length -eq 0) {
            throw "Installed package asset is missing or empty: $path"
        }
    }
    if (Test-Path -LiteralPath (Join-Path $runtimeDir "openbb-runtime")) {
        throw "Installed package contains the obsolete OpenBB runtime path."
    }

    $installedTree = @(
        Get-Item -LiteralPath $installRoot -Force
        Get-ChildItem -LiteralPath $installRoot -Recurse -Force
    )
    $reparsePoints = @(
        $installedTree |
            Where-Object { ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0 }
    )
    if ($reparsePoints.Count -ne 0) {
        throw "Installed package contains a reparse point."
    }
    $localBuildMetadata = @(
        Get-ChildItem -LiteralPath $runtimeDir -Filter "direct_url.json" -Recurse -File -Force |
            Where-Object { $_.Directory.Name.EndsWith(".dist-info", [StringComparison]::Ordinal) }
    )
    if ($localBuildMetadata.Count -ne 0) {
        throw "Installed runtime contains local build-path metadata."
    }

    $openbbNativeFiles = @(
        Get-ChildItem -LiteralPath $openbbRuntime -Recurse -File -Force |
            Where-Object { $_.Extension -in @(".exe", ".dll", ".pyd") } |
            ForEach-Object { $_.FullName }
    )
    if ($openbbNativeFiles.Count -eq 0) {
        throw "Installed OpenBB runtime contains no native Windows files."
    }
    $installedNativeBinaries = @($mainBinary, $coreBinary, $piBinary, $financeBinary, $computeBinary)
    $installedNativeBinaries += $openbbNativeFiles
    foreach ($binary in $installedNativeBinaries) {
        Assert-X64Pe $binary
    }
    $mainVersion = (Get-Item -LiteralPath $mainBinary).VersionInfo
    if ($mainVersion.ProductName -ne $productName -or
        $mainVersion.FileDescription -ne $productName -or
        $mainVersion.CompanyName -ne $publisher -or
        $mainVersion.ProductVersion -ne $ExpectedVersion) {
        throw "Installed app identity or version resources are invalid."
    }

    $uninstallIdentity = Get-ItemProperty -LiteralPath $uninstallRegistryPath
    if ($uninstallIdentity.DisplayName -ne $productName -or
        $uninstallIdentity.DisplayVersion -ne $ExpectedVersion -or
        $uninstallIdentity.Publisher -ne $publisher -or
        ([string]$uninstallIdentity.InstallLocation).Trim('"') -ne $installRoot) {
        throw "Installed app registration identity is invalid."
    }

    $sourceAgentDir = Join-Path $appRoot "agent"
    $sourceAgentFiles = @(Get-ChildItem -LiteralPath $sourceAgentDir -Recurse -File -Force)
    $installedAgentFiles = @(Get-ChildItem -LiteralPath $agentDir -Recurse -File -Force)
    if ($sourceAgentFiles.Count -ne $installedAgentFiles.Count) {
        throw "Installed agent tree differs from the source tree."
    }
    foreach ($sourceFile in $sourceAgentFiles) {
        $relative = [IO.Path]::GetRelativePath($sourceAgentDir, $sourceFile.FullName)
        Assert-SameFile $sourceFile.FullName (Join-Path $agentDir $relative)
    }
    Assert-SameFile (Join-Path $repoRoot "LICENSE") (Join-Path $installRoot "LICENSE")
    Assert-SameFile (Join-Path $repoRoot "NOTICE") (Join-Path $installRoot "NOTICE")
    Assert-SameFile (Join-Path $appRoot "THIRD_PARTY_NOTICES.md") (Join-Path $installRoot "THIRD_PARTY_NOTICES.md")

    $piVersion = "0.84.2"
    $denoVersion = "2.9.5"
    $pyodideVersion = "314.0.3"
    if ((Get-Content -LiteralPath (Join-Path $runtimeDir ".pi-version") -Raw).Trim() -ne $piVersion -or
        (Get-Content -LiteralPath (Join-Path $runtimeDir "package.json") -Raw | ConvertFrom-Json).version -ne $piVersion) {
        throw "Installed Pi runtime identity is invalid."
    }
    $piSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $piBinary).Hash.ToLowerInvariant()
    if ($piSha256 -ne (Get-Content -LiteralPath (Join-Path $runtimeDir ".pi-executable.sha256") -Raw).Trim()) {
        throw "Installed Pi binary differs from its pinned digest."
    }
    if ((Get-Content -LiteralPath (Join-Path $computeRuntime ".deno-version") -Raw).Trim() -ne $denoVersion -or
        (Get-Content -LiteralPath (Join-Path $computeRuntime ".pyodide-version") -Raw).Trim() -ne $pyodideVersion) {
        throw "Installed compute runtime identity is invalid."
    }
    $computeSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $computeBinary).Hash.ToLowerInvariant()
    if ($computeSha256 -ne (Get-Content -LiteralPath (Join-Path $computeRuntime ".compute-executable.sha256") -Raw).Trim()) {
        throw "Installed compute binary differs from its pinned digest."
    }
    $computeManifestSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $computeRuntime "runtime-manifest.json")).Hash.ToLowerInvariant()
    if ($computeManifestSha256 -ne (Get-Content -LiteralPath (Join-Path $computeRuntime ".compute-manifest.sha256") -Raw).Trim()) {
        throw "Installed compute manifest differs from its pinned digest."
    }

    $coreVersionLine = Select-String -LiteralPath (Join-Path $repoRoot "Cargo.toml") -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if (-not $coreVersionLine -or $coreVersionLine.Matches[0].Groups[1].Value -ne $ExpectedVersion) {
        throw "Core source version differs from the installed package version."
    }
    Invoke-CheckedProcess $pythonBinary @(
        (Join-Path $scriptDir "check-sidecars.py"),
        "--pi", $piBinary,
        "--pi-version", $piVersion,
        "--pi-runtime", $runtimeDir,
        "--provider-extension", (Join-Path $agentDir "guruterminal-provider-extension.mjs"),
        "--core", $coreBinary,
        "--core-version", $ExpectedVersion,
        "--finance", $financeBinary,
        "--compute", $computeBinary,
        "--compute-runtime", $computeRuntime,
        "--deno-version", $denoVersion,
        "--pyodide-version", $pyodideVersion,
        "--openbb", $openbbBinary,
        "--openbb-runtime", $openbbRuntime
    ) 300

    if ($RequireDistributionSigning) {
        $signedBinaries = @($mainBinary, $coreBinary, $piBinary, $financeBinary, $computeBinary, $uninstaller)
        $signedBinaries += $openbbNativeFiles
        foreach ($binary in $signedBinaries) {
            Assert-Authenticode $binary
        }
    }
} catch {
    $primaryError = $_
} finally {
    try {
        $uninstaller = Join-Path $installRoot "uninstall.exe"
        if ($installStarted -and (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
            Invoke-CheckedProcess $uninstaller @("/S", "/NS") 300
        }
        if (Test-Path -LiteralPath $productRegistryPath) {
            $productRegistryKey = Get-Item -LiteralPath $productRegistryPath
            $savedInstallRoot = [string]$productRegistryKey.GetValue("")
            if ($savedInstallRoot.Trim('"') -eq $installRoot) {
                Remove-Item -LiteralPath $productRegistryPath -Recurse -Force
            }
        }
        if (-not $publisherRegistryExisted -and (Test-Path -LiteralPath $publisherRegistryPath)) {
            $publisherRegistryKey = Get-Item -LiteralPath $publisherRegistryPath
            if ($publisherRegistryKey.SubKeyCount -eq 0 -and $publisherRegistryKey.ValueCount -eq 0) {
                Remove-Item -LiteralPath $publisherRegistryPath -Force
            }
        }
        if ($installStarted -and
            ((Test-Path -LiteralPath $installRoot) -or
             (Test-Path -LiteralPath $uninstallRegistryPath) -or
             (Test-Path -LiteralPath $productRegistryPath))) {
            throw "Final package check did not remove the isolated installation."
        }
    } catch {
        $cleanupError = $_
    }
}

if ($null -ne $primaryError) {
    if ($null -ne $cleanupError) {
        Write-Warning "Package cleanup also failed: $($cleanupError.Exception.Message)"
    }
    throw $primaryError
}
if ($null -ne $cleanupError) {
    throw $cleanupError
}

Write-Host "Guru Terminal final Windows NSIS package contents passed."
