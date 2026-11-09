<#
Builds QuietDesk and connects it to Claude Code for every session. Safe to run again, for example after a code change.
The mode itself is not changed: it stays as it was, and is off on a first install. Undo everything with uninstall.ps1.

A running keeper is restarted, so that it and the commands speak the same way. With the mode on that replugs the virtual monitor, with
the connect sound and a second's freeze in a full-screen game, and a test on the monitor loses it for a moment: not during your
game, and not while a session uses `quietdesk monitor`.

  %LOCALAPPDATA%\QuietDesk\bin  the programs (first on your user PATH, so Claude Code finds the pwsh.exe shim)
  ~\.claude\settings.json     env.CLAUDE_CODE_SHELL_PREFIX, for the Bash tool (a copy of the old file is kept once)
  ~\.claude\CLAUDE.md         the rule that the mode never reduces testing
  ~\.claude\skills\quietdesk   the skill: how to use it and what differs when hidden
  HKCU\...\Run                 the tray icon, at sign-in

Claude Code reads PATH when the app starts: restart the Claude app once after the first install.
#>
. (Join-Path $PSScriptRoot 'common.ps1')

$project = Split-Path -Parent $PSScriptRoot
$programs = @('pwsh.exe', 'quietdesk-bash.exe', 'quietdesk.exe', 'quietdesk-tray.exe')

Push-Location $project
try {
    & cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE." }
}
finally { Pop-Location }

New-Item -ItemType Directory -Force -Path $bin | Out-Null
foreach ($old in @(Get-ChildItem -LiteralPath $bin -Filter '*.replaced-*' -ErrorAction SilentlyContinue)) {
    Remove-Item -LiteralPath $old.FullName -ErrorAction SilentlyContinue
}
$stamp = Get-Date -Format yyyyMMddHHmmssfff

# PowerShell 7 as Claude Code finds it with no shim: on PATH, then where Claude Code looks. With none it uses Windows PowerShell 5.1,
# and a shim named pwsh would make it believe it has 7, so the PowerShell tool is left as it is, and not hidden.
$realPowerShell = @(
    @($env:Path -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $bin } | ForEach-Object { Join-Path $_ 'pwsh.exe' })
    (Join-Path $env:ProgramFiles 'PowerShell\7\pwsh.exe')
    (Join-Path $env:LOCALAPPDATA 'Microsoft\WindowsApps\pwsh.exe')
    (Join-Path $env:USERPROFILE '.dotnet\tools\pwsh.exe')
) | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $realPowerShell) {
    $programs = @($programs | Where-Object { $_ -ne 'pwsh.exe' })
    $shim = Join-Path $bin 'pwsh.exe'
    if (Test-Path -LiteralPath $shim) { Rename-Item -LiteralPath $shim -NewName "pwsh.exe.replaced-$stamp" }
    Write-Warning 'No PowerShell 7 found, so Claude Code uses Windows PowerShell 5.1, which QuietDesk leaves as it is: the PowerShell tool is not hidden. Install PowerShell 7, then run this again.'
}

# A running program cannot be overwritten but can be renamed, so sessions in the middle of a command keep working. Every new copy is
# put beside its program first, and then each is swapped in with two renames: a program is missing only between those two. If a
# swap fails, the programs already swapped go back too, so they never come from two builds.
foreach ($program in $programs) {
    Copy-Item -LiteralPath (Join-Path $project "target\release\$program") -Destination (Join-Path $bin "$program.new") -Force
}
$swapped = [Collections.Generic.List[object]]::new()
try {
    foreach ($program in $programs) {
        $target = Join-Path $bin $program
        $replaced = if (Test-Path -LiteralPath $target) { "$program.replaced-$stamp" } else { $null }
        if ($replaced) { Rename-Item -LiteralPath $target -NewName $replaced }
        $swapped.Add([pscustomobject]@{ Program = $program; Replaced = $replaced })
        Rename-Item -LiteralPath "$target.new" -NewName $program
    }
}
catch {
    for ($index = $swapped.Count - 1; $index -ge 0; $index--) {
        $entry = $swapped[$index]
        $target = Join-Path $bin $entry.Program
        if (Test-Path -LiteralPath $target) { Rename-Item -LiteralPath $target -NewName "$($entry.Program).replaced-failed-$stamp" }
        if ($entry.Replaced) { Rename-Item -LiteralPath (Join-Path $bin $entry.Replaced) -NewName $entry.Program }
    }
    foreach ($program in $programs) { Remove-Item -LiteralPath (Join-Path $bin "$program.new") -ErrorAction SilentlyContinue }
    throw
}

Set-QuietDeskOnUserPath $true

# The machine PATH comes before the user PATH: a PowerShell installed there would be found before the shim.
$machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
foreach ($entry in @($machinePath -split ';' | Where-Object { $_ -and $realPowerShell })) {
    if (Test-Path -LiteralPath (Join-Path $entry 'pwsh.exe')) {
        Write-Warning "PowerShell in the machine PATH ($entry) comes before the shim: the PowerShell tool will not be hidden."
    }
}

# Settings: one value, with a copy of the file from before the first install.
$backup = Join-Path $state 'settings.before-install.json'
$settings = if (Test-Path -LiteralPath $settingsPath) { Get-Content -Raw -LiteralPath $settingsPath | ConvertFrom-Json } else { [pscustomobject]@{} }
if ((Test-Path -LiteralPath $settingsPath) -and -not (Test-Path -LiteralPath $backup)) {
    Copy-Item -LiteralPath $settingsPath -Destination $backup
}
if (-not $settings.PSObject.Properties['env']) { $settings | Add-Member -NotePropertyName env -NotePropertyValue ([pscustomobject]@{}) }
$settings.env | Add-Member -NotePropertyName CLAUDE_CODE_SHELL_PREFIX -NotePropertyValue $prefixValue -Force
Write-ClaudeSettings $settings

# The rule, between markers so a later install replaces it and uninstall removes only it.
$rule = (Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'claude-rule.md')).TrimEnd()
$block = "$ruleStart`n$rule`n$ruleEnd"
$existing = if (Test-Path -LiteralPath $claudeMd) { Get-Content -Raw -LiteralPath $claudeMd } else { '' }
$pattern = "(?s)$([regex]::Escape($ruleStart)).*?$([regex]::Escape($ruleEnd))"
$updated = if ($existing -match $pattern) { [regex]::Replace($existing, $pattern, { $block }) } elseif ($existing) { "$($existing.TrimEnd())`n`n$block`n" } else { "$block`n" }
[IO.File]::WriteAllText($claudeMd, $updated, [Text.UTF8Encoding]::new($false))

# The skill, for every project.
New-Item -ItemType Directory -Force -Path $skillDir | Out-Null
Copy-Item -LiteralPath (Join-Path $project 'skill\SKILL.md') -Destination (Join-Path $skillDir 'SKILL.md') -Force

# A keeper runs until the mode is off, from the file replaced above. The tray below plugs the monitor back in when the mode is on.
foreach ($keeper in @(Get-CimInstance Win32_Process -Filter "Name = 'quietdesk.exe'" | Where-Object { $_.CommandLine -match 'monitor-keeper' })) {
    Stop-Process -Id $keeper.ProcessId -Force
    Wait-Process -Id $keeper.ProcessId -Timeout 5 -ErrorAction SilentlyContinue
}

# The tray icon: at sign-in, and now. A running one was replaced above, so it is restarted to run the new build.
$tray = Join-Path $bin 'quietdesk-tray.exe'
Set-ItemProperty -LiteralPath $runKey -Name $runValue -Value "`"$tray`""
Stop-QuietDeskTray
# On the real desktop even from a command that runs hidden, apart from this script, and as sign-in starts it.
& (Join-Path $bin 'quietdesk.exe') start-tray
if ($LASTEXITCODE -ne 0) { throw "quietdesk start-tray failed with exit code $LASTEXITCODE." }
# Its status line asks whether the tray runs, and with the mode on the tray plugs the monitor in, the path a sign-in takes too. The
# keeper gives up within 13 s, 8 for a monitor the killed keeper left and 5 for the new display, so 15 s is its answer either way.
$plugDeadline = (Get-Date).AddSeconds(15)
do {
    Start-Sleep -Seconds 1
    $status = @(& (Join-Path $bin 'quietdesk.exe') status)
    if ($LASTEXITCODE -ne 0) { throw "quietdesk status failed with exit code $LASTEXITCODE." }
    $notPlugged = ($status -join "`n") -match 'NOT plugged in'
} while ($notPlugged -and (Get-Date) -lt $plugDeadline)

Write-Host "Installed to $bin"
$status
if ($notPlugged) { throw 'The mode is on, but the tray did not plug the virtual monitor in. See problems.log in the state folder.' }
if (($status -join "`n") -match 'MORE THAN ONE') { throw 'A virtual monitor stayed beside the new one, where it can catch the mouse. See quietdesk status.' }
Write-Host 'Restart the Claude app once, so its sessions find the new PATH.'
