Add-MpPreference -ExclusionPath "C:\Users\jiehu\.rustup"
if ($LASTEXITCODE -eq 0) {
    Write-Host "Success: Added exclusion for C:\Users\jiehu\.rustup"
} else {
    Write-Host "Failed with exit code: $LASTEXITCODE"
}
