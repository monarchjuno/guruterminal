[CmdletBinding()]
param(
    [switch]$Full,
    [ValidateSet("seed", "verify")]
    [string]$PersistencePhase,
    [string]$StateRoot,
    [string]$ImportRoot
)

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
$NativePersistence = Join-Path $ScriptDir "native-persistence.mjs"
$TauriCli = Join-Path $AppRoot "node_modules/@tauri-apps/cli/tauri.js"
$TauriConfig = Join-Path $AppRoot "src-tauri/tauri.e2e.conf.json"
$RemoveStateRoot = $false
$TauriProcess = $null
$TauriOutputCapture = $null
$EmitLauncherDiagnostics = $false
$HttpHandler = $null
$HttpClient = $null
$LauncherLogTailLineCount = 80
$LauncherLogDrainTimeoutMilliseconds = 5000

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

function Dispose-Quietly([System.IDisposable]$Resource) {
    if ($null -eq $Resource) {
        return
    }
    try {
        $Resource.Dispose()
    }
    catch {
    }
}

function Start-ProcessOutputCapture(
    [System.Diagnostics.Process]$Process,
    [string]$StandardOutputPath,
    [string]$StandardErrorPath
) {
    $standardOutputStream = $null
    $standardErrorStream = $null
    $standardOutputSource = $null
    $standardErrorSource = $null

    try {
        $standardOutputStream = [System.IO.FileStream]::new(
            $StandardOutputPath,
            [System.IO.FileMode]::Create,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::Read
        )
        $standardErrorStream = [System.IO.FileStream]::new(
            $StandardErrorPath,
            [System.IO.FileMode]::Create,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::Read
        )
        $standardOutputSource = $Process.StandardOutput.BaseStream
        $standardErrorSource = $Process.StandardError.BaseStream

        # Pump both redirected streams concurrently so either pipe can fill
        # without blocking the launcher. CopyToAsync has a fixed-size buffer.
        $standardOutputTask = $standardOutputSource.CopyToAsync(
            $standardOutputStream,
            81920
        )
        $standardErrorTask = $standardErrorSource.CopyToAsync(
            $standardErrorStream,
            81920
        )

        return [pscustomobject]@{
            Process = $Process
            StandardOutputPath = $StandardOutputPath
            StandardErrorPath = $StandardErrorPath
            StandardOutputSource = $standardOutputSource
            StandardErrorSource = $standardErrorSource
            StandardOutputStream = $standardOutputStream
            StandardErrorStream = $standardErrorStream
            StandardOutputTask = $standardOutputTask
            StandardErrorTask = $standardErrorTask
            Completed = $false
        }
    }
    catch {
        Dispose-Quietly $standardOutputSource
        Dispose-Quietly $standardErrorSource
        Dispose-Quietly $standardOutputStream
        Dispose-Quietly $standardErrorStream
        throw
    }
}

function Complete-ProcessOutputCapture([object]$Capture) {
    if ($null -eq $Capture -or $Capture.Completed) {
        return
    }

    try {
        if ($Capture.Process.HasExited) {
            $tasks = [System.Threading.Tasks.Task[]]@(
                $Capture.StandardOutputTask,
                $Capture.StandardErrorTask
            )
            $drainTask = [System.Threading.Tasks.Task]::WhenAll($tasks)
            $drained = $false
            try {
                $drained = $drainTask.Wait($LauncherLogDrainTimeoutMilliseconds)
            }
            catch {
                # A failed copy has already completed; retain its partial log.
                $drained = $true
            }
            if (-not $drained) {
                Write-Warning (
                    "Guru Terminal launcher output did not close within " +
                    "$LauncherLogDrainTimeoutMilliseconds milliseconds; printing the captured tail."
                )
            }
        }
    }
    finally {
        Dispose-Quietly $Capture.StandardOutputSource
        Dispose-Quietly $Capture.StandardErrorSource
        try {
            $Capture.StandardOutputStream.Flush()
        }
        catch {
        }
        try {
            $Capture.StandardErrorStream.Flush()
        }
        catch {
        }
        Dispose-Quietly $Capture.StandardOutputStream
        Dispose-Quietly $Capture.StandardErrorStream
        $Capture.Completed = $true
    }
}

function Write-LauncherLogTail(
    [string]$Label,
    [string]$Path,
    [int]$LineCount
) {
    Write-Host "Guru Terminal launcher $Label log tail (up to $LineCount lines): $Path"
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        Write-Host "(log was not created)"
        return
    }

    try {
        $lines = @(Get-Content -LiteralPath $Path -Tail $LineCount -ErrorAction Stop)
    }
    catch {
        Write-Warning "Could not read Guru Terminal launcher $Label log."
        return
    }

    if ($lines.Count -eq 0) {
        Write-Host "(no output captured)"
        return
    }
    foreach ($line in $lines) {
        Write-Host $line
    }
}

function Write-LauncherLogTails([object]$Capture) {
    if ($null -eq $Capture) {
        return
    }
    Write-LauncherLogTail "standard output" $Capture.StandardOutputPath $LauncherLogTailLineCount
    Write-LauncherLogTail "standard error" $Capture.StandardErrorPath $LauncherLogTailLineCount
}

function Wait-ForOwnedWebDriver(
    [int]$Port,
    [int]$RootProcessId,
    [int]$TimeoutMilliseconds
) {
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMilliseconds)
    while ([DateTime]::UtcNow -lt $deadline) {
        if (-not (Test-ProcessRunning $RootProcessId)) {
            $script:EmitLauncherDiagnostics = $true
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
    $script:EmitLauncherDiagnostics = $true
    throw (
        "Guru Terminal WebDriver did not become ready and owned within " +
        "$TimeoutMilliseconds milliseconds (listening PIDs: $listeners)."
    )
}

function Stop-StartedProcessTree([System.Diagnostics.Process]$RootProcess) {
    if ($null -eq $RootProcess) {
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
    [int]$WebDriverPort,
    [string]$ImportDirectory
) {
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $NodeBinary
    $startInfo.WorkingDirectory = $AppRoot
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $startInfo.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)

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
    if (-not [string]::IsNullOrWhiteSpace($ImportDirectory)) {
        $startInfo.Environment["GURUTERMINAL_E2E_IMPORT_DIR"] = $ImportDirectory
    }
    $startInfo.Environment["TAURI_WEBDRIVER_PORT"] = [string]$WebDriverPort
    return $startInfo
}

if ($Full -and -not [string]::IsNullOrWhiteSpace($PersistencePhase)) {
    throw "-Full cannot be combined with -PersistencePhase."
}
if ([string]::IsNullOrWhiteSpace($PersistencePhase)) {
    if (
        -not [string]::IsNullOrWhiteSpace($StateRoot) -or
        -not [string]::IsNullOrWhiteSpace($ImportRoot)
    ) {
        throw "-StateRoot and -ImportRoot require -PersistencePhase."
    }
}
else {
    $StateRoot = Resolve-RealAbsoluteDirectory $StateRoot "-StateRoot"
    $ImportRoot = Resolve-RealAbsoluteDirectory $ImportRoot "-ImportRoot"
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

    if ([string]::IsNullOrWhiteSpace($StateRoot)) {
        $StateRoot = Join-Path ([System.IO.Path]::GetTempPath()) (
            "guruterminal-e2e-" + [Guid]::NewGuid().ToString("N")
        )
        $RemoveStateRoot = $true
    }
    $AppDataDirectory = Join-Path $StateRoot "app-data"
    New-Item -ItemType Directory -Path $AppDataDirectory -Force | Out-Null

    $HttpHandler = [System.Net.Http.HttpClientHandler]::new()
    $HttpHandler.UseProxy = $false
    $HttpClient = [System.Net.Http.HttpClient]::new($HttpHandler)
    $HttpClient.Timeout = [TimeSpan]::FromMilliseconds(500)

    Write-Host "Guru Terminal E2E profile: com.monarchjuno.guruterminal.e2e"
    Write-Host "Guru Terminal E2E state: $AppDataDirectory"
    Write-Host "Guru Terminal E2E WebDriver: http://127.0.0.1:$WebDriverPort"

    $processInfo = New-CleanTauriProcessInfo `
        $NodeBinary `
        $AppDataDirectory `
        $WebDriverPort `
        $ImportRoot
    $TauriProcess = [System.Diagnostics.Process]::Start($processInfo)
    if ($null -eq $TauriProcess) {
        throw "Guru Terminal E2E launcher did not start."
    }

    $TauriOutputCapture = Start-ProcessOutputCapture `
        $TauriProcess `
        (Join-Path $ArtifactDir "native-windows-launcher.stdout.log") `
        (Join-Path $ArtifactDir "native-windows-launcher.stderr.log")

    Wait-ForOwnedWebDriver $WebDriverPort $TauriProcess.Id $StartupTimeout
    & $NodeBinary $WaitSession `
        --write-session $SessionInfo `
        --pid $TauriProcess.Id `
        --port $WebDriverPort `
        --profile e2e
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to write the Guru Terminal E2E session."
    }

    if ([string]::IsNullOrWhiteSpace($PersistencePhase)) {
        $smokeArgs = @($SessionInfo)
        if ($Full) {
            $smokeArgs += "--full"
        }
        & $NodeBinary $NativeSmoke @smokeArgs
        if ($LASTEXITCODE -ne 0) {
            $EmitLauncherDiagnostics = $true
            throw "Guru Terminal native Windows smoke failed."
        }
        Write-Host "Guru Terminal native Windows smoke passed."
    }
    else {
        & $NodeBinary $NativePersistence $SessionInfo $PersistencePhase
        if ($LASTEXITCODE -ne 0) {
            $EmitLauncherDiagnostics = $true
            throw "Guru Terminal native Windows persistence $PersistencePhase phase failed."
        }
        Write-Host "Guru Terminal native Windows persistence $PersistencePhase phase passed."
    }
}
finally {
    if ($null -ne $TauriProcess) {
        Stop-StartedProcessTree $TauriProcess
    }
    if ($null -ne $TauriOutputCapture) {
        Complete-ProcessOutputCapture $TauriOutputCapture
        if ($EmitLauncherDiagnostics) {
            Write-LauncherLogTails $TauriOutputCapture
        }
    }
    if ($null -ne $HttpClient) {
        $HttpClient.Dispose()
    }
    if ($null -ne $HttpHandler) {
        $HttpHandler.Dispose()
    }
    Remove-Item -LiteralPath $SessionInfo -Force -ErrorAction SilentlyContinue
    if ($RemoveStateRoot -and $null -ne $StateRoot) {
        Remove-Item -LiteralPath $StateRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}
