$ErrorActionPreference = "Stop"
Add-MpPreference -ExclusionPath "C:\Users\jiehu\dev\tagent\target" -ErrorAction Stop
Write-Host "Exclusion added"
