$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$piVersion = "0.84.2"
$archiveSha256 = "741fc1ae1afecb573ac2888e011188ff446b3940f4aabe1583f60bf55be8a3d0"
$archiveUrl = "https://github.com/earendil-works/pi/releases/download/v$piVersion/pi-windows-x64.zip"
$target = "x86_64-pc-windows-msvc"

$scriptDir = $PSScriptRoot
$appRoot = Split-Path -Parent $scriptDir
$tauriRoot = Join-Path $appRoot "src-tauri"
$runtimeDir = Join-Path $tauriRoot "resources\pi-runtime"
$temporaryRoot = Join-Path ([IO.Path]::GetTempPath()) ("guruterminal-pi-" + [guid]::NewGuid())
$archive = Join-Path $temporaryRoot "pi-windows-x64.zip"
$extracted = Join-Path $temporaryRoot "extracted"
$newRuntime = Join-Path (Split-Path -Parent $runtimeDir) (".pi-runtime-" + [guid]::NewGuid())

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "Pi staging requires 64-bit Windows."
}

New-Item -ItemType Directory -Force -Path $temporaryRoot, $extracted, $newRuntime | Out-Null
try {
    if ($env:GURUTERMINAL_PI_ARCHIVE) {
        Copy-Item -LiteralPath $env:GURUTERMINAL_PI_ARCHIVE -Destination $archive
    } else {
        Invoke-WebRequest -Uri $archiveUrl -OutFile $archive
    }

    $actualSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive).Hash.ToLowerInvariant()
    if ($actualSha256 -ne $archiveSha256) {
        throw "Pi archive checksum mismatch."
    }

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $zip = [IO.Compression.ZipFile]::OpenRead($archive)
    try {
        $entryNames = @($zip.Entries | ForEach-Object { $_.FullName })
        foreach ($entryName in $entryNames) {
            $segments = $entryName -split "[/\\]"
            if ([IO.Path]::IsPathRooted($entryName) -or $segments -contains "..") {
                throw "Pi archive contains an unsafe path: $entryName"
            }
        }
        if ($entryNames -notcontains "pi.exe" -or $entryNames -notcontains "package.json") {
            throw "Pi archive is missing its executable or package metadata."
        }
    } finally {
        $zip.Dispose()
    }

    Expand-Archive -LiteralPath $archive -DestinationPath $extracted
    $piExecutable = Join-Path $extracted "pi.exe"
    $reportedVersion = (& $piExecutable --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $reportedVersion -ne $piVersion) {
        throw "Pi executable version does not match the pinned version."
    }

    Copy-Item -Path (Join-Path $extracted "*") -Destination $newRuntime -Recurse -Force
    Set-Content -LiteralPath (Join-Path $newRuntime ".pi-version") -Value $piVersion -Encoding ascii
    Set-Content -LiteralPath (Join-Path $newRuntime ".pi-archive.sha256") -Value $archiveSha256 -Encoding ascii

    $upstreamRuntimePi = Join-Path $newRuntime "pi.exe"
    $runtimePi = Join-Path $newRuntime "guruterminal-pi.exe"
    Move-Item -LiteralPath $upstreamRuntimePi -Destination $runtimePi
    & (Join-Path $scriptDir "sign-windows-binary.ps1") -Path $runtimePi
    $executableSha256 = (Get-FileHash -Algorithm SHA256 -LiteralPath $runtimePi).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $newRuntime ".pi-executable.sha256") -Value $executableSha256 -Encoding ascii

    if (Test-Path -LiteralPath $runtimeDir) {
        Remove-Item -LiteralPath $runtimeDir -Recurse -Force
    }
    Move-Item -LiteralPath $newRuntime -Destination $runtimeDir
    $newRuntime = $null
} finally {
    if ($newRuntime -and (Test-Path -LiteralPath $newRuntime)) {
        Remove-Item -LiteralPath $newRuntime -Recurse -Force
    }
    if (Test-Path -LiteralPath $temporaryRoot) {
        Remove-Item -LiteralPath $temporaryRoot -Recurse -Force
    }
}

Write-Host "Staged Pi v$piVersion for $target."
