param(
    [Parameter(Mandatory = $true)]
    [string]$Path
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $env:GURUTERMINAL_WINDOWS_CERTIFICATE_THUMBPRINT) {
    Write-Host "Windows distribution signing is not configured; leaving $Path unsigned."
    return
}

$resolvedPath = (Resolve-Path -LiteralPath $Path).Path
$timestampUrl = if ($env:GURUTERMINAL_WINDOWS_TIMESTAMP_URL) {
    $env:GURUTERMINAL_WINDOWS_TIMESTAMP_URL
} else {
    "https://timestamp.digicert.com"
}

$signTool = Get-Command signtool.exe -ErrorAction SilentlyContinue
if (-not $signTool) {
    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $signTool = Get-ChildItem -Path $kitsRoot -Filter signtool.exe -Recurse |
        Where-Object { $_.FullName -match "\\x64\\signtool\.exe$" } |
        Sort-Object FullName -Descending |
        Select-Object -First 1
}
if (-not $signTool) {
    throw "signtool.exe is required for Windows distribution signing."
}
$signToolPath = if ($signTool.PSObject.Properties.Name -contains "Source") {
    $signTool.Source
} else {
    $signTool.FullName
}

& $signToolPath sign `
    /sha1 $env:GURUTERMINAL_WINDOWS_CERTIFICATE_THUMBPRINT `
    /fd SHA256 `
    /tr $timestampUrl `
    /td SHA256 `
    $resolvedPath
if ($LASTEXITCODE -ne 0) {
    throw "signtool failed to sign $resolvedPath"
}

& $signToolPath verify /pa /v $resolvedPath
if ($LASTEXITCODE -ne 0) {
    throw "Authenticode verification failed for $resolvedPath"
}
