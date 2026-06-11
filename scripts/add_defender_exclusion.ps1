$ErrorActionPreference = "Stop"
Add-MpPreference -ExclusionPath (Join-Path $PSScriptRoot "target") -ErrorAction Stop
Write-Host "Exclusion added"
