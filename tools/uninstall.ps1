<#
Undoes install.ps1: turns the mode off (which gives back your console setting), removes the tray icon, the PATH entry, the
settings value, the rule and the skill. The programs stay in %LOCALAPPDATA%\QuietDesk until you pass -RemoveFiles: sessions that are running keep
using them until they restart, and with the mode off they only pass commands through. Restart the Claude app first.
#>
param([switch]$RemoveFiles)
. (Join-Path $PSScriptRoot 'common.ps1')

$quietdesk = Join-Path $bin 'quietdesk.exe'
if (Test-Path -LiteralPath $quietdesk) {
    & $quietdesk off
    if ($LASTEXITCODE -ne 0) { throw "quietdesk off failed with exit code $LASTEXITCODE; nothing else was changed." }
}

Remove-ItemProperty -LiteralPath $runKey -Name $runValue -ErrorAction SilentlyContinue
Stop-QuietDeskTray
Set-QuietDeskOnUserPath $false

if (Test-Path -LiteralPath $settingsPath) {
    $settings = Get-Content -Raw -LiteralPath $settingsPath | ConvertFrom-Json
    $environmentSettings = if ($settings.PSObject.Properties['env']) { $settings.env } else { $null }
    if ($environmentSettings -and $environmentSettings.PSObject.Properties['CLAUDE_CODE_SHELL_PREFIX'] -and
        $environmentSettings.CLAUDE_CODE_SHELL_PREFIX -eq $prefixValue) {
        $environmentSettings.PSObject.Properties.Remove('CLAUDE_CODE_SHELL_PREFIX')
        Write-ClaudeSettings $settings
    }
}

if (Test-Path -LiteralPath $claudeMd) {
    $pattern = "(?s)\s*$([regex]::Escape($ruleStart)).*?$([regex]::Escape($ruleEnd))"
    $left = [regex]::Replace((Get-Content -Raw -LiteralPath $claudeMd), $pattern, '').Trim()
    if ($left) { [IO.File]::WriteAllText($claudeMd, "$left`n", [Text.UTF8Encoding]::new($false)) } else { Remove-Item -LiteralPath $claudeMd }
}

if (Test-Path -LiteralPath $skillDir) { Remove-Item -LiteralPath $skillDir -Recurse -Force }

# Programs Windows may start muted, after a test of them ended while muted: with QuietDesk gone, nothing gives their sound back.
$muted = @(Get-Content -LiteralPath (Join-Path $state 'muted-programs') -ErrorAction SilentlyContinue | Where-Object { $_ })
if ($muted.Count) {
    Write-Warning "Windows may start these muted next time, from test runs: unmute them in the Volume mixer if so. $($muted -join '; ')"
}

if ($RemoveFiles) {
    # With the mode off the keeper unplugs the monitor and ends a few seconds after its last command; it runs from this folder.
    foreach ($keeper in @(Get-CimInstance Win32_Process -Filter "Name = 'quietdesk.exe'" | Where-Object { $_.CommandLine -match 'monitor-keeper' })) {
        Wait-Process -Id $keeper.ProcessId -Timeout 30 -ErrorAction SilentlyContinue
        if (Get-Process -Id $keeper.ProcessId -ErrorAction SilentlyContinue) { Stop-Process -Id $keeper.ProcessId -Force; Wait-Process -Id $keeper.ProcessId -Timeout 5 -ErrorAction SilentlyContinue }
    }
    Remove-Item -LiteralPath $state -Recurse -Force
    Write-Host "Removed $state"
} else {
    Write-Host "Disconnected. After restarting the Claude app, run this again with -RemoveFiles to delete $state"
}
