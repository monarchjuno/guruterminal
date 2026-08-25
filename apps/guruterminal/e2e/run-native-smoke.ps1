[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $IsWindows -or -not [Environment]::Is64BitOperatingSystem) {
    throw "Guru Terminal native Windows smoke requires 64-bit Windows PowerShell 7."
}

$ScriptDir = Split-Path -Parent $PSCommandPath
$AppRoot = Split-Path -Parent $ScriptDir
$ArtifactDir = Join-Path $ScriptDir "artifacts"
$SessionInfo = Join-Path $ArtifactDir "current-session.json"
$WaitSession = Join-Path $ScriptDir "wait-session.mjs"
$NativeSmoke = Join-Path $ScriptDir "native-smoke.mjs"
$TauriCli = Join-Path $AppRoot "node_modules/@tauri-apps/cli/tauri.js"
$TauriConfig = Join-Path $AppRoot "src-tauri/tauri.e2e.conf.json"
$StateRoot = $null
$TauriProcess = $null
$HttpHandler = $null
$HttpClient = $null

function Require-Application([string]$Name) {
    $command = @(
        Get-Command $Name -CommandType Application -ErrorAction Stop |
            Select-Object -First 1
    )[0]
    if ([string]::IsNullOrWhiteSpace($command.Path)) {
        throw "required command is missing: $Name"
    }
    return $command.Path
}

function Read-PositiveIntegerEnvironment([string]$Name, [int]$Fallback) {
    $value = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($value)) {
        return $Fallback
    }

    $parsed = 0
    if (-not [int]::TryParse($value, [ref]$parsed) -or $parsed -le 0) {
        throw "$Name must be a positive integer."
    }
    return $parsed
}

function Resolve-RequestedPort([string]$NodeBinary) {
    $requested = [Environment]::GetEnvironmentVariable("GURUTERMINAL_E2E_PORT")
    if ([string]::IsNullOrWhiteSpace($requested)) {
        $listener = [System.Net.Sockets.TcpListener]::new(
            [System.Net.IPAddress]::Loopback,
            0
        )
        try {
            $listener.Start()
            $requested = [string]$listener.LocalEndpoint.Port
        }
        finally {
            $listener.Stop()
        }
    }

    $port = 0
    if (
        -not [int]::TryParse($requested, [ref]$port) -or
        $port -lt 1024 -or
        $port -gt 65535
    ) {
        throw "GURUTERMINAL_E2E_PORT must be an integer from 1024 to 65535."
    }

    $resolved = & $NodeBinary $WaitSession --resolve-port $port
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to resolve a safe Guru Terminal E2E WebDriver port."
    }

    $resolvedPort = 0
    if (-not [int]::TryParse(($resolved | Select-Object -Last 1), [ref]$resolvedPort)) {
        throw "The WebDriver port resolver returned an invalid port."
    }
    return $resolvedPort
}

function Test-ProcessRunning([int]$ProcessId) {
    try {
        return -not (Get-Process -Id $ProcessId -ErrorAction Stop).HasExited
    }
    catch {
        return $false
    }
}

function Get-ProcessTreePostOrder([int]$RootProcessId) {
    $seen = [System.Collections.Generic.HashSet[int]]::new()
    $postOrder = [System.Collections.Generic.List[int]]::new()
    $pending = [System.Collections.Generic.Stack[object]]::new()
    $pending.Push([pscustomobject]@{ Id = $RootProcessId; Expanded = $false })

    while ($pending.Count -gt 0) {
        $entry = $pending.Pop()
        $processId = [int]$entry.Id
        if ($entry.Expanded) {
            [void]$postOrder.Add($processId)
            continue
        }
        if (-not $seen.Add($processId)) {
            continue
        }

        $pending.Push([pscustomobject]@{ Id = $processId; Expanded = $true })
        $children = @(
            Get-CimInstance -ClassName Win32_Process `
                -Filter "ParentProcessId = $processId" `
                -ErrorAction Stop |
                ForEach-Object { [int]$_.ProcessId }
        )
        foreach ($childId in $children) {
            if ($childId -gt 0) {
                $pending.Push([pscustomobject]@{ Id = $childId; Expanded = $false })
            }
        }
    }

    return $postOrder.ToArray()
}

function Get-ListeningProcessIds([int]$Port) {
    try {
        return @(
            Get-NetTCPConnection -State Listen -LocalPort $Port -ErrorAction Stop |
                ForEach-Object { [int]$_.OwningProcess } |
                Select-Object -Unique
        )
    }
    catch {
        return @()
    }
}

function Test-ListenerOwnedByProcessTree([int]$Port, [int]$RootProcessId) {
    try {
        $tree = Get-ProcessTreePostOrder $RootProcessId
    }
    catch {
        return $false
    }
    $listeners = Get-ListeningProcessIds $Port
    return @($listeners | Where-Object { $tree -contains $_ }).Count -gt 0
}

function Test-WebDriverReady([int]$Port) {
    try {
        $request = $script:HttpClient.GetStringAsync("http://127.0.0.1:$Port/status")
        $document = $request.GetAwaiter().GetResult()
        $status = $document | ConvertFrom-Json
        return $status.value.ready -eq $true
    }
    catch {
        return $false
    }
}

function Wait-ForOwnedWebDriver(
    [int]$Port,
    [int]$RootProcessId,
    [int]$TimeoutMilliseconds
) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Test-ProcessRunning $RootProcessId)) {
            throw "Guru Terminal launcher exited before WebDriver became ready."
        }
        if (
            (Test-WebDriverReady $Port) -and
            (Test-ListenerOwnedByProcessTree $Port $RootProcessId)
        ) {
            return
        }
        Start-Sleep -Milliseconds 200
    }

    $listeners = (Get-ListeningProcessIds $Port) -join ", "
    throw (
        "Guru Terminal WebDriver did not become ready and owned within " +
        "$TimeoutMilliseconds milliseconds (listening PIDs: $listeners)."
    )
}

function Stop-StartedProcessTree([System.Diagnostics.Process]$RootProcess) {
    if ($null -eq $RootProcess -or $RootProcess.HasExited) {
        return
    }

    try {
        $processIds = Get-ProcessTreePostOrder $RootProcess.Id
    }
    catch {
        Write-Warning "Could not inspect the Guru Terminal E2E process tree; stopping only its launcher."
        $processIds = @($RootProcess.Id)
    }

    foreach ($processId in $processIds) {
        Stop-Process -Id $processId -Force -ErrorAction SilentlyContinue
    }

    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    while ([DateTime]::UtcNow -lt $deadline) {
        $remaining = @(
            $processIds | Where-Object { Test-ProcessRunning ([int]$_) }
        )
        if ($remaining.Count -eq 0) {
            return
        }
        Start-Sleep -Milliseconds 100
    }
    Write-Warning "Guru Terminal E2E process tree did not exit before cleanup timed out."
}

function New-CleanTauriProcessInfo(
    [string]$NodeBinary,
    [string]$AppDataDirectory,
    [int]$WebDriverPort
) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $NodeBinary
    $startInfo.WorkingDirectory = $AppRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true

    foreach ($argument in @(
        $TauriCli,
        "dev",
        "--no-watch",
        "--features",
        "e2e",
        "--config",
        $TauriConfig
    )) {
        [void]$startInfo.ArgumentList.Add($argument)
    }

    # Keep the embedded WebDriver out of any ambient CI credential, proxy, or
    # Node/Rust injection environment. The allowlist retains only Windows,
    # Rust, and MSVC runtime locations necessary to compile and launch Tauri.
    $startInfo.Environment.Clear()
    $inheritedNames = @(
        "Path",
        "SystemRoot",
        "WINDIR",
        "ComSpec",
        "PATHEXT",
        "SystemDrive",
        "TEMP",
        "TMP",
        "HOME",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "VCINSTALLDIR",
        "VCToolsInstallDir",
        "VCToolsRedistDir",
        "VSINSTALLDIR",
        "WindowsSdkDir",
        "WindowsSDKLibVersion",
        "WindowsSDKVersion",
        "UniversalCRTSdkDir",
        "UCRTVersion",
        "Platform",
        "PROCESSOR_ARCHITECTURE",
        "PROCESSOR_ARCHITEW6432",
        "NUMBER_OF_PROCESSORS"
    )
    foreach ($name in $inheritedNames) {
        $value = [Environment]::GetEnvironmentVariable($name)
        if (-not [string]::IsNullOrWhiteSpace($value)) {
            $startInfo.Environment[$name] = $value
        }
    }
    $startInfo.Environment["GURUTERMINAL_E2E_APP_DATA_DIR"] = $AppDataDirectory
    $startInfo.Environment["TAURI_WEBDRIVER_PORT"] = [string]$WebDriverPort
    return $startInfo
}

try {
    $NodeBinary = Require-Application "node"
    [void](Require-Application "npm")
    [void](Require-Application "cargo")
    if (-not (Test-Path -LiteralPath $TauriCli -PathType Leaf)) {
        throw "Guru Terminal dependencies are not installed. Run: (cd apps/guruterminal && npm ci)"
    }

    $StartupTimeout = Read-PositiveIntegerEnvironment "GURUTERMINAL_E2E_STARTUP_TIMEOUT_MS" 300000
    $WebDriverPort = Resolve-RequestedPort $NodeBinary
    New-Item -ItemType Directory -Path $ArtifactDir -Force | Out-Null
    Remove-Item -LiteralPath $SessionInfo -Force -ErrorAction SilentlyContinue

    $StateRoot = Join-Path ([System.IO.Path]::GetTempPath()) (
        "guruterminal-e2e-" + [Guid]::NewGuid().ToString("N")
    )
    $AppDataDirectory = Join-Path $StateRoot "app-data"
    New-Item -ItemType Directory -Path $AppDataDirectory -Force | Out-Null

    $HttpHandler = [System.Net.Http.HttpClientHandler]::new()
    $HttpHandler.UseProxy = $false
    $HttpClient = [System.Net.Http.HttpClient]::new($HttpHandler)
    $HttpClient.Timeout = [TimeSpan]::FromMilliseconds(500)

    Write-Host "Guru Terminal E2E profile: com.monarchjuno.guruterminal.e2e"
    Write-Host "Guru Terminal E2E state: $AppDataDirectory"
    Write-Host "Guru Terminal E2E WebDriver: http://127.0.0.1:$WebDriverPort"

    $processInfo = New-CleanTauriProcessInfo $NodeBinary $AppDataDirectory $WebDriverPort
    $TauriProcess = [System.Diagnostics.Process]::Start($processInfo)
    if ($null -eq $TauriProcess) {
        throw "Guru Terminal E2E launcher did not start."
    }

    Wait-ForOwnedWebDriver $WebDriverPort $TauriProcess.Id $StartupTimeout
    & $NodeBinary $WaitSession `
        --write-session $SessionInfo `
        --pid $TauriProcess.Id `
        --port $WebDriverPort `
        --profile e2e
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to write the Guru Terminal E2E session."
    }

    & $NodeBinary $NativeSmoke $SessionInfo
    if ($LASTEXITCODE -ne 0) {
        throw "Guru Terminal native Windows smoke failed."
    }
    Write-Host "Guru Terminal native Windows smoke passed."
}
finally {
    if ($null -ne $TauriProcess) {
        Stop-StartedProcessTree $TauriProcess
    }
    if ($null -ne $HttpClient) {
        $HttpClient.Dispose()
    }
    if ($null -ne $HttpHandler) {
        $HttpHandler.Dispose()
    }
    Remove-Item -LiteralPath $SessionInfo -Force -ErrorAction SilentlyContinue
    if ($null -ne $StateRoot) {
        Remove-Item -LiteralPath $StateRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
