$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

& (Join-Path $PSScriptRoot "stage-pi-windows-x64.ps1")
& (Join-Path $PSScriptRoot "stage-core-windows-x64.ps1")
& (Join-Path $PSScriptRoot "stage-finance-windows-x64.ps1")
& (Join-Path $PSScriptRoot "stage-compute-windows-x64.ps1")
& (Join-Path $PSScriptRoot "stage-openbb-windows-x64.ps1")
& (Join-Path $PSScriptRoot "check-package-prerequisites.ps1")

Write-Host "Guru Terminal Windows x64 staging is complete."
