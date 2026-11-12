<#
Proves the installed mode end to end: a real, throwaway Claude Code session (a cheap model running exact commands) with the
installed programs, the user's settings and a mode file of its own, so running sessions keep theirs. Its findings come from
files the commands write and from what is running, never from what the model says.

Run first with the mode off, then on, and compares them: a command must behave the same either way, on another desktop.
Checks: Bash and PowerShell land on the right desktop, the working directory and exit codes carry through, a flip reaches the
next command, `quietdesk visible` reaches the real screen, a timed-out command really stops, a new console stays off the screen,
MCP servers connect the same, and a command on the virtual monitor carries its exit code and leaves the monitor as your mode says.

It briefly sets new consoles to the classic console, as `quietdesk on` does, and gives the setting back.
#>
. (Join-Path $PSScriptRoot 'common.ps1')

$quietdesk = Join-Path $bin 'quietdesk.exe'
# From a hidden shell, as any session has with your mode on: with this run's mode off its commands would stay hidden, and its
# watcher would count hidden windows as yours. It proves the real desktop, so it runs from there.
$desktop = @(& $quietdesk desktop)
if ($LASTEXITCODE -ne 0) { throw "quietdesk desktop failed with exit code $LASTEXITCODE." }
if ($desktop[0] -ne 'Default') {
    & $quietdesk visible pwsh -NoProfile -File $PSCommandPath
    exit $LASTEXITCODE
}
# The CLI on PATH, else the newest one the Claude app keeps.
$claude = @(Get-Command claude -CommandType Application -ErrorAction SilentlyContinue | ForEach-Object Source) +
    @(Get-ChildItem -LiteralPath (Join-Path $env:APPDATA 'Claude\claude-code') -Recurse -Filter 'claude.exe' -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTime -Descending | ForEach-Object FullName) | Select-Object -First 1
if (-not $claude) { throw 'Claude Code was not found: neither `claude` on PATH nor the Claude app.' }
$work = Join-Path ([IO.Path]::GetTempPath()) "quietdesk-verify-$PID"
New-Item -ItemType Directory -Force -Path (Join-Path $work 'sub') | Out-Null
$results = Join-Path $work 'results.txt'
$bashResults = $results -replace '\\', '/'
$failures = [Collections.Generic.List[string]]::new()

function Test-That([string]$Name, [bool]$Passed, [string]$Detail = '') {
    Write-Host ("[{0}] {1}{2}" -f $(if ($Passed) { 'PASS' } else { 'FAIL' }), $Name, $(if ($Detail) { ": $Detail" } else { '' }))
    if (-not $Passed) { $failures.Add($Name) }
}

$env:PATH = "$bin;$env:PATH"
$env:QUIETDESK_DIR = $work
Remove-Item env:QUIETDESK_DECIDED -ErrorAction SilentlyContinue
foreach ($name in @(Get-ChildItem env: | Where-Object { $_.Name -match '^CLAUDE' } | ForEach-Object Name)) { Remove-Item "env:$name" }
$env:CLAUDE_CODE_USE_POWERSHELL_TOOL = '1'

# Runs one prompt; returns the session's whole event stream as text.
function Invoke-Session([string]$Prompt, [int]$MaxTurns) {
    Push-Location $work
    try {
        $stream = $null | & $claude -p $Prompt --model claude-haiku-5-5 --allowedTools 'Bash,PowerShell' --max-turns $MaxTurns --output-format stream-json --verbose 2>&1
        return ($stream | ForEach-Object { "$_" }) -join "`n"
    }
    finally { Pop-Location }
}

function Get-Results {
    $map = @{}
    if (Test-Path -LiteralPath $results) {
        foreach ($line in @(Get-Content -LiteralPath $results)) {
            $at = $line.IndexOf('=')
            if ($at -gt 0) { $map[$line.Substring(0, $at)] = $line.Substring($at + 1).Trim() }
        }
        Remove-Item -LiteralPath $results
    }
    return $map
}

function Get-McpStatus([string]$Stream) {
    $init = @($Stream -split "`n" | Where-Object { $_ -match '"subtype":"init"' }) | Select-Object -First 1
    if (-not $init) { return 'no init event' }
    $started = $init | ConvertFrom-Json
    return (@($started.mcp_servers | Sort-Object name | ForEach-Object { "$($_.name):$($_.status)" }) -join ' ')
}

$commonSteps = @"
Run these one at a time, each as its own tool call, exactly as written, without changing them, then reply only with the word done.
1. Bash: echo "bash-desktop=`$(quietdesk desktop)" >> '$bashResults'
2. PowerShell: "ps-desktop=`$(quietdesk desktop)" | Add-Content -LiteralPath '$results'
3. Bash: cd sub && echo "bash-cd-here=`$(basename "`$PWD")" >> '$bashResults'
4. Bash: echo "bash-cd-next=`$(basename "`$PWD")" >> '$bashResults'
5. Bash: cd ..
6. PowerShell: Set-Location sub; "ps-cd-here=`$(Split-Path -Leaf (Get-Location))" | Add-Content -LiteralPath '$results'
7. PowerShell: "ps-cd-next=`$(Split-Path -Leaf (Get-Location))" | Add-Content -LiteralPath '$results'
8. PowerShell: Set-Location ..
9. Bash: echo marker-bash-out; exit 7
10. PowerShell: Write-Output marker-ps-out; exit 9
"@

# This process's copy of the console setting as it was, backup included: it must be exactly so again at the end.
function Get-ConsoleSetting {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Console\%%Startup')
    $before = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Console\%%Startup\QuietDeskBefore')
    $values = if ($key) { @($key.GetValueNames() | Sort-Object | ForEach-Object { "$_=$($key.GetValue($_))" }) } else { @() }
    return "$($values -join '; ') | backup: $([bool]$before)"
}
$consoleAtStart = Get-ConsoleSetting

Write-Host '--- the mode off: the baseline'
& $quietdesk off | Out-Null
if ($LASTEXITCODE -ne 0) { throw "quietdesk off failed with exit code $LASTEXITCODE." }
$offStream = Invoke-Session $commonSteps 16
$off = Get-Results
Test-That 'off: Bash runs on the real desktop' ($off['bash-desktop'] -eq 'Default') $off['bash-desktop']
Test-That 'off: PowerShell runs on the real desktop' ($off['ps-desktop'] -eq 'Default') $off['ps-desktop']

Write-Host '--- the mode on'
& $quietdesk on | Out-Null
if ($LASTEXITCODE -ne 0) { throw "quietdesk on failed with exit code $LASTEXITCODE." }
try {
    $onStream = Invoke-Session $commonSteps 16
    $on = Get-Results
    Test-That 'on: Bash runs hidden' ($on['bash-desktop'] -eq 'QuietDesk') $on['bash-desktop']
    Test-That 'on: PowerShell runs hidden' ($on['ps-desktop'] -eq 'QuietDesk') $on['ps-desktop']
    foreach ($key in 'bash-cd-here', 'bash-cd-next', 'ps-cd-here', 'ps-cd-next') {
        Test-That "same with the mode on: $key" ($null -ne $on[$key] -and $on[$key] -eq $off[$key]) "off=$($off[$key]) on=$($on[$key])"
    }
    foreach ($text in 'marker-bash-out', 'marker-ps-out', 'Exit code 7', 'Exit code 9') {
        Test-That "off shows $text" ($offStream.Contains($text))
        Test-That "on shows $text" ($onStream.Contains($text))
    }
    $mcpOff = Get-McpStatus $offStream
    $mcpOn = Get-McpStatus $onStream
    Test-That 'MCP servers connect the same' ($mcpOff -eq $mcpOn -and $mcpOn -ne 'no init event') "off: $mcpOff | on: $mcpOn"

    Write-Host '--- flips, the real screen, timeouts, a new console'
    $probeBat = Join-Path $work 'console-probe.bat'
    # The new console writes where it runs: a window that had already gone by the time it is looked for proves nothing.
    $consoleDesktop = Join-Path $work 'console-desktop.txt'
    Set-Content -LiteralPath $probeBat -Encoding ascii -Value '@start "QuietDeskConsoleProbe" cmd.exe /c "quietdesk desktop >"%~dp0console-desktop.txt" & ping -n 6 127.0.0.1 >nul"'
    $psPid = Join-Path $work 'ps.pid'
    $bashPid = (Join-Path $work 'bash.pid') -replace '\\', '/'
    $terminalsBefore = @(Get-Process -Name WindowsTerminal -ErrorAction SilentlyContinue).Count
    [void](Invoke-Session @"
Run these one at a time, each as its own tool call, exactly as written, without changing them, then reply only with the word done. Two of them will time out: that is expected, go on with the next.
1. Bash: quietdesk off
2. Bash: echo "after-off=`$(quietdesk desktop)" >> '$bashResults'
3. PowerShell: "after-off-ps=`$(quietdesk desktop)" | Add-Content -LiteralPath '$results'
4. Bash: quietdesk on
5. PowerShell: "after-on-ps=`$(quietdesk desktop)" | Add-Content -LiteralPath '$results'
6. Bash: echo "visible=`$(quietdesk visible quietdesk desktop)" >> '$bashResults'
7. PowerShell, with the timeout parameter set to 5000: `$PID | Set-Content -LiteralPath '$psPid'; Start-Sleep -Seconds 300
8. Bash, with the timeout parameter set to 5000: cat /proc/`$`$/winpid > '$bashPid'; exec sleep 300
9. PowerShell: & '$probeBat'
"@ 16)
    $flips = Get-Results
    Test-That 'a flip to off reaches the next Bash command' ($flips['after-off'] -eq 'Default') $flips['after-off']
    Test-That 'a flip to off reaches the next PowerShell command' ($flips['after-off-ps'] -eq 'Default') $flips['after-off-ps']
    Test-That 'a flip to on reaches the next command' ($flips['after-on-ps'] -eq 'QuietDesk') $flips['after-on-ps']
    Test-That 'quietdesk visible reaches the real screen from hidden' ($flips['visible'] -eq 'Default') $flips['visible']

    $seen = @(Get-Process | Where-Object { $_.MainWindowTitle -match 'QuietDeskConsoleProbe' })
    Test-That 'a new console stays off the screen' ($seen.Count -eq 0) "$($seen.Count) windows"
    $deadline = (Get-Date).AddSeconds(10)
    while (-not (Test-Path -LiteralPath $consoleDesktop) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
    $where = if (Test-Path -LiteralPath $consoleDesktop) { (Get-Content -Raw -LiteralPath $consoleDesktop).Trim() } else { 'it never ran' }
    Test-That 'a new console runs hidden' ($where -eq 'QuietDesk') $where
    Test-That 'a new console does not open Windows Terminal' (@(Get-Process -Name WindowsTerminal -ErrorAction SilentlyContinue).Count -le $terminalsBefore)

    Start-Sleep -Seconds 2
    foreach ($item in @(@{ Name = 'PowerShell'; File = $psPid }, @{ Name = 'Bash'; File = ($bashPid -replace '/', '\') })) {
        if (Test-Path -LiteralPath $item.File) {
            $id = [int](Get-Content -LiteralPath $item.File).Trim()
            $alive = [bool](Get-Process -Id $id -ErrorAction SilentlyContinue)
            Test-That "a timed-out $($item.Name) command really stops" (-not $alive) "pid $id"
            if ($alive) { Stop-Process -Id $id -Force }
        } else {
            Test-That "a timed-out $($item.Name) command really stops" $false 'it never recorded its process id'
        }
    }

    Write-Host '--- a command on the virtual monitor, through the PowerShell tool'
    $monitorStatus = (& $quietdesk status | Out-String)
    if ($LASTEXITCODE -ne 0) { throw "quietdesk status failed with exit code $LASTEXITCODE." }
    [void](Invoke-Session @"
Run this one command with the PowerShell tool, exactly as written, then reply only with the word done.
quietdesk monitor cmd.exe /c "exit 5"; "monitor-exit=`$LASTEXITCODE" | Add-Content -LiteralPath '$results'
"@ 4)
    $monitorResult = Get-Results
    Test-That 'a command on the virtual monitor carries its exit code' ($monitorResult['monitor-exit'] -eq '5') "exit $($monitorResult['monitor-exit'])"
    if ($monitorStatus -match 'driver not installed') {
        Write-Host '[SKIP] the virtual monitor plugging in and out: its driver is not installed'
    }
    else {
        # Your own mode decides: on, it holds the monitor whatever this session's mode file says.
        $yoursIsOn = "$(Get-Content -Raw -LiteralPath (Join-Path $state 'mode') -ErrorAction SilentlyContinue)".Trim() -eq 'on'
        & $quietdesk off | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "quietdesk off failed with exit code $LASTEXITCODE." }
        if ($yoursIsOn) {
            Start-Sleep -Seconds 6
            Test-That 'the virtual monitor stays plugged in: your mode is on' ((& $quietdesk status | Out-String) -match 'Virtual monitor: in use')
        }
        else {
            $deadline = (Get-Date).AddSeconds(30)
            do {
                $status = (& $quietdesk status | Out-String)
                Start-Sleep -Milliseconds 500
            } while ($status -notmatch 'Virtual monitor: not in use' -and (Get-Date) -lt $deadline)
            Test-That 'the virtual monitor unplugs itself with the mode off' ($status -match 'Virtual monitor: not in use')
        }
    }
}
finally {
    & $quietdesk off | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Warning 'quietdesk off failed: check the console setting with quietdesk status.' }
}
$consoleAtEnd = Get-ConsoleSetting
Test-That 'the console setting came back' ($consoleAtEnd -eq $consoleAtStart) "before: $consoleAtStart; after: $consoleAtEnd"

Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
if ($failures.Count) {
    Write-Host "FAILED: $($failures.Count) check(s)"
    exit 1
}
Write-Host 'All checks passed.'
