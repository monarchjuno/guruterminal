[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "Guru Terminal native Windows persistence requires 64-bit Windows PowerShell 7."
}

$ScriptDir = Split-Path -Parent $PSCommandPath
$Launcher = Join-Path $ScriptDir "run-native-smoke.ps1"
$ImportRoot = Join-Path $ScriptDir "fixtures/Imported Memory E2E"
$StateRoot = $null

function Resolve-RealAbsoluteDirectory([string]$Path, [string]$Name) {
    if ([string]::IsNullOrWhiteSpace($Path) -or -not [System.IO.Path]::IsPathRooted($Path)) {
        throw "$Name must be an absolute directory."
    }

    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if (-not $item.PSIsContainer -or [bool]($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
        throw "$Name must be a real directory."
    }

    return $item.FullName
}

function Wait-ForTauriDevServerExit {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    $ownerProcessIds = @()
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $ownerProcessIds = @(
                Get-NetTCPConnection -State Listen -LocalPort 1420 -ErrorAction Stop |
                    ForEach-Object { [int]$_.OwningProcess } |
                    Select-Object -Unique
            )
        }
        catch {
            $ownerProcessIds = @()
        }
        if ($ownerProcessIds.Count -eq 0) {
            return
        }
        Start-Sleep -Milliseconds 100
    }

    throw (
        "Guru Terminal dev server did not exit within 15 seconds " +
        "(listening PIDs: $($ownerProcessIds -join ', '))."
    )
}

try {
    if (-not (Test-Path -LiteralPath $Launcher -PathType Leaf)) {
        throw "Guru Terminal native Windows launcher is missing: $Launcher"
    }
    $ImportRoot = Resolve-RealAbsoluteDirectory $ImportRoot "Native Memory import fixture"

    $StateRoot = Join-Path ([System.IO.Path]::GetTempPath()) (
        "guruterminal-persistence-e2e-" + [Guid]::NewGuid().ToString("N")
    )
    New-Item -ItemType Directory -Path $StateRoot | Out-Null

    foreach ($phase in @("seed", "verify")) {
        & $Launcher `
            -PersistencePhase $phase `
            -StateRoot $StateRoot `
            -ImportRoot $ImportRoot
        if ($LASTEXITCODE -ne 0) {
            throw "Guru Terminal native Windows persistence $phase phase failed."
        }
        Wait-ForTauriDevServerExit
    }

    Write-Host "Guru Terminal native Windows persistence smoke passed."
}
finally {
    if ($null -ne $StateRoot -and (Test-Path -LiteralPath $StateRoot)) {
        try {
            $stateItem = Get-Item -LiteralPath $StateRoot -Force -ErrorAction Stop
            if ([bool]($stateItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint)) {
                Write-Warning "Refusing to remove reparse-point persistence state: $StateRoot"
            }
            else {
                Remove-Item -LiteralPath $StateRoot -Recurse -Force -ErrorAction Stop
            }
        }
        catch {
            Write-Warning "Could not remove native Windows persistence state: $StateRoot"
        }
    }
}
