# Create AppLocker rule for rustup folder
$rule = New-AppLockerPolicy -RulePath "C:\Users\jiehu\.rustup" -RuleType Path -User Everyone -Action Allow -ErrorAction SilentlyContinue

if ($rule) {
    Write-Host "Rule created successfully"
} else {
    Write-Host "Creating rule with XML..."
}

# Alternative: Import a custom AppLocker policy XML
$xml = @"
<?xml version="1.0" encoding="utf-8"?>
<AppLockerPolicy Version="1">
  <RuleCollection Type="Exe" EnforcementMode="Enabled">
    <FilePathRule Id="{7cd6f5c1-5efa-4b7e-8e7a-1a2b3c4d5e6f}" Name="Allow rustup" Description="Allow rustup executables" UserOrGroupSid="S-1-1-0" Action="Allow">
      <Conditions>
        <FilePathCondition Path="%USERPROFILE%\.rustup\*">
      </Conditions>
    </FilePathRule>
  </RuleCollection>
</AppLockerPolicy>
"@

$tempFile = "$env:TEMP\applocker_rustup.xml"
$xml | Out-File -FilePath $tempFile -Encoding utf8 -Force

Set-AppLockerPolicy -XmlPolicyFile $tempFile -ErrorAction SilentlyContinue
if ($LASTEXITCODE -eq 0) {
    Write-Host "AppLocker policy set successfully"
} else {
    Write-Host "Failed to set policy. Trying alternative method..."
}

Remove-Item $tempFile -Force -ErrorAction SilentlyContinue

# Also check if we can enable the service
$svc = Get-Service -Name "AppIDSvc" -ErrorAction SilentlyContinue
if ($svc) {
    Write-Host "AppIDSvc status: $($svc.Status)"
}
