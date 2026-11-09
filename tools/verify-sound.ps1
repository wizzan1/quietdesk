<#
Proves the sound guard with a real tone and Windows' own audio session state, against the installed programs and the running tray:
a hidden run is muted, window or no window, and new hidden sound is muted before it can be heard; the same program launched normally gets its sound at once and leaves the list; a copy already running
is not touched and does not clear the list; with the option off nothing is muted; a guard killed mid-mute repairs itself; other
apps are left alone. It opens small windows, can be heard for a moment, restarts the tray, and with the mode off plugs in the virtual
monitor, so not during your game.
#>
. (Join-Path $PSScriptRoot 'common.ps1')
$here = Join-Path ([IO.Path]::GetTempPath()) "quietdesk-sound-$PID"
New-Item -ItemType Directory -Force -Path $here | Out-Null
$ps51 = "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
$ps51x86 = "$env:WINDIR\SysWOW64\WindowsPowerShell\v1.0\powershell.exe"
$failures = 0
function Report([string]$Name, [bool]$Passed, [string]$Detail = '') {
    "[{0}] {1}{2}" -f $(if ($Passed) { 'PASS' } else { 'FAIL' }), $Name, $(if ($Detail) { ": $Detail" } else { '' })
    if (-not $Passed) { $script:failures++ }
}

# A quiet 440 Hz second, played in a loop by an ordinary window.
$wav = Join-Path $here 'tone.wav'
$rate = 44100
$samples = [int16[]]::new($rate)
for ($i = 0; $i -lt $rate; $i++) { $samples[$i] = [int16](1400 * [math]::Sin(2 * [math]::PI * 440 * $i / $rate)) }
$stream = [IO.File]::Create($wav); $writer = [IO.BinaryWriter]::new($stream)
$writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $writer.Write([int](36 + $rate * 2)); $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt '))
$writer.Write([int]16); $writer.Write([int16]1); $writer.Write([int16]1); $writer.Write([int]$rate); $writer.Write([int]($rate * 2)); $writer.Write([int16]2); $writer.Write([int16]16)
$writer.Write([Text.Encoding]::ASCII.GetBytes('data')); $writer.Write([int]($rate * 2))
foreach ($sample in $samples) { $writer.Write($sample) }
$writer.Dispose()
$toneScript = Join-Path $here 'tone.ps1'
Set-Content -LiteralPath $toneScript -Encoding ascii -Value @"
param([string]`$PidFile, [int]`$Seconds)
Add-Type -AssemblyName System.Windows.Forms, System
`$PID | Set-Content -LiteralPath `$PidFile
`$player = New-Object System.Media.SoundPlayer '$wav'
`$player.PlayLooping()
`$form = New-Object System.Windows.Forms.Form
`$form.Text = 'QuietDeskToneTest'; `$form.Width = 300; `$form.Height = 120; `$form.ShowInTaskbar = `$false
`$timer = New-Object System.Windows.Forms.Timer
`$timer.Interval = `$Seconds * 1000
`$timer.Add_Tick({ `$player.Stop(); `$form.Close() })
`$timer.Start()
[void]`$form.ShowDialog()
"@
# The same tone from a program with no window, and started by the Store's PowerShell 7, as the PowerShell tool would.
$quietScript = Join-Path $here 'quiet.ps1'
Set-Content -LiteralPath $quietScript -Encoding ascii -Value @"
param([string]`$PidFile, [int]`$Seconds)
`$PID | Set-Content -LiteralPath `$PidFile
`$player = New-Object System.Media.SoundPlayer '$wav'
`$player.PlayLooping()
Start-Sleep -Seconds `$Seconds
`$player.Stop()
"@
$chainScript = Join-Path $here 'chain.ps1'
Set-Content -LiteralPath $chainScript -Encoding ascii -Value @"
param([string]`$PidFile, [int]`$Seconds)
& '$ps51' -NoProfile -ExecutionPolicy Bypass -File '$quietScript' `$PidFile `$Seconds
"@

Add-Type -TypeDefinition @'
using System; using System.Collections.Generic; using System.Diagnostics; using System.Runtime.InteropServices;
[ComImport, Guid("BCDE0395-E52F-467C-8E3D-C4579291692E")] class MMDeviceEnumeratorCom {}
[ComImport, Guid("A95664D2-9614-4F35-A746-DE8DB63617E6"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)] interface IMMDeviceEnumerator {
    int EnumAudioEndpoints(int flow, int mask, out IntPtr devices);
    [PreserveSig] int GetDefaultAudioEndpoint(int flow, int role, out IMMDevice device);
}
[ComImport, Guid("D666063F-1587-4E43-81F1-B948E807363F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)] interface IMMDevice {
    [PreserveSig] int Activate(ref Guid iid, int context, IntPtr parameters, [MarshalAs(UnmanagedType.IUnknown)] out object result);
}
[ComImport, Guid("77AA99A0-1BD6-484F-8BC7-2C654C9A9B6F"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)] interface IAudioSessionManager2 {
    int GetAudioSessionControl(IntPtr a, int b, out IntPtr c);
    int GetSimpleAudioVolume(IntPtr a, int b, out IntPtr c);
    [PreserveSig] int GetSessionEnumerator(out IAudioSessionEnumerator sessions);
}
[ComImport, Guid("E2F5BB11-0570-40CA-ACDD-3AA01277DEE8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)] interface IAudioSessionEnumerator {
    [PreserveSig] int GetCount(out int count);
    [PreserveSig] int GetSession(int index, [MarshalAs(UnmanagedType.IUnknown)] out object session);
}
[ComImport, Guid("bfb7ff88-7239-4fc9-8fa2-07c950be9c6d"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)] interface IAudioSessionControl2 {
    int GetState(out int s); int GetDisplayName(out IntPtr a); int SetDisplayName(IntPtr a, IntPtr b); int GetIconPath(out IntPtr a); int SetIconPath(IntPtr a, IntPtr b);
    int GetGroupingParam(out Guid g); int SetGroupingParam(ref Guid g, IntPtr c); int RegisterAudioSessionNotification(IntPtr a); int UnregisterAudioSessionNotification(IntPtr a);
    int GetSessionIdentifier(out IntPtr a); int GetSessionInstanceIdentifier(out IntPtr a);
    [PreserveSig] int GetProcessId(out uint pid);
}
[ComImport, Guid("87CE5498-68D6-44E5-9215-6DA47EF883D8"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)] interface ISimpleAudioVolume {
    int SetMasterVolume(float level, ref Guid context); int GetMasterVolume(out float level); int SetMute(bool mute, ref Guid context);
    [PreserveSig] int GetMute(out bool mute);
}
public class SessionState { public uint Pid; public string Name; public bool Muted; }
public static class Sessions {
    public static List<SessionState> All() {
        var list = new List<SessionState>();
        var enumerator = (IMMDeviceEnumerator)new MMDeviceEnumeratorCom();
        IMMDevice device; Marshal.ThrowExceptionForHR(enumerator.GetDefaultAudioEndpoint(0, 1, out device));
        Guid id = typeof(IAudioSessionManager2).GUID; object managerObject;
        Marshal.ThrowExceptionForHR(device.Activate(ref id, 23, IntPtr.Zero, out managerObject));
        IAudioSessionEnumerator sessions; ((IAudioSessionManager2)managerObject).GetSessionEnumerator(out sessions);
        int count; sessions.GetCount(out count);
        for (int i = 0; i < count; i++) {
            object session; sessions.GetSession(i, out session);
            var control = session as IAudioSessionControl2; uint pid;
            if (control == null || control.GetProcessId(out pid) != 0 || pid == 0) continue;
            string name; try { name = Process.GetProcessById((int)pid).ProcessName; } catch (ArgumentException) { continue; }
            bool muted = false; var volume = session as ISimpleAudioVolume; if (volume != null) volume.GetMute(out muted);
            list.Add(new SessionState { Pid = pid, Name = name, Muted = muted });
        }
        return list;
    }
}
'@

# Starts the tone with a quietdesk word in front; returns its process id once it has a session.
function Start-Tone([string]$Word, [int]$Seconds, [string]$Tag, [string]$Script = $toneScript, [string]$Shell = $ps51, [bool]$InsideJob = $false) {
    $file = Join-Path $here "tone-$Tag.pid"
    if (Test-Path -LiteralPath $file) { Remove-Item -LiteralPath $file }
    $toneArguments = "-NoProfile -STA -ExecutionPolicy Bypass -File `"$Script`" `"$file`" $Seconds"
    $process = if ($InsideJob) { Start-InsideJob "`"$(Join-Path $bin 'quietdesk.exe')`" $Word `"$Shell`" $toneArguments" }
    else { Start-Process -FilePath 'quietdesk' -ArgumentList $Word, "`"$Shell`"", $toneArguments -PassThru -WindowStyle Hidden }
    $deadline = (Get-Date).AddSeconds(15)
    while ((Get-Date) -lt $deadline) {
        if (Test-Path -LiteralPath $file) {
            $id = [int](Get-Content -LiteralPath $file | Select-Object -First 1)
            if (@([Sessions]::All() | Where-Object { $_.Pid -eq $id }).Count) { return [pscustomobject]@{ Id = $id; Wrapper = $process } }
        }
        Start-Sleep -Milliseconds 50
    }
    throw "the tone never got an audio session ($Tag)"
}
# Mute state of a session over time: was it ever heard, and how did it end?
function Watch-Mute([int]$Id, [int]$Milliseconds) {
    $seen = [Collections.Generic.List[bool]]::new()
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($clock.ElapsedMilliseconds -lt $Milliseconds) {
        foreach ($session in @([Sessions]::All() | Where-Object { $_.Pid -eq $Id })) { $seen.Add($session.Muted) }
        Start-Sleep -Milliseconds 50
    }
    return [pscustomobject]@{ Samples = $seen.Count; MutedSamples = @($seen | Where-Object { $_ }).Count; Last = $(if ($seen.Count) { $seen[$seen.Count - 1] } else { $null }) }
}
function Stop-Tone($Tone) {
    Stop-Process -Id $Tone.Id -Force -ErrorAction SilentlyContinue
    if (-not $Tone.Wrapper.HasExited) { [void]$Tone.Wrapper.WaitForExit(3000) }
}

function Set-Muting([string]$Value) {
    & quietdesk mute $Value | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "quietdesk mute $Value failed with exit code $LASTEXITCODE." }
}
# Each test program launched once on the real screen, as the user would: the guard gives it its sound back and drops it from the list.
function Restore-TestPrograms([switch]$Report) {
    foreach ($shell in $ps51, $ps51x86) {
        $tone = Start-Tone 'visible' 25 "r$(Split-Path -Leaf (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $shell))))" $toneScript $shell
        Start-Sleep -Milliseconds 800
        $heard = @([Sessions]::All() | Where-Object { $_.Pid -eq $tone.Id -and -not $_.Muted }).Count -eq 1
        if ($Report) { Report "your launch of $shell has its sound" $heard }
        Stop-Tone $tone
        Start-Sleep -Seconds 1
    }
}
# A run that stopped half way can leave the test programs listed, and an entry already there would hide this run's own.
$listedAtStart = @(Get-Content -LiteralPath (Join-Path $state 'muted-programs') -ErrorAction SilentlyContinue)
if (@($listedAtStart | Where-Object { $_ -in $ps51, $ps51x86 }).Count) {
    'the test programs are still listed from an earlier run: launching each once, as you would, to clear them'
    Restore-TestPrograms
}
# What this check put on the list: real test runs may have left other programs there, and they stay until those are next launched.
$listedBefore = @(Get-Content -LiteralPath (Join-Path $state 'muted-programs') -ErrorAction SilentlyContinue)
if (@($listedBefore | Where-Object { $_ -in $ps51, $ps51x86 }).Count) { throw 'The test programs stayed on the muted list after a launch on the real screen: the guard is not repairing.' }
function Read-MutedList { return (@(Get-Content -LiteralPath (Join-Path $state 'muted-programs') -ErrorAction SilentlyContinue) | Where-Object { $_ -and $listedBefore -notcontains $_ }) -join "`n" }
$othersBefore = @([Sessions]::All() | Where-Object { $_.Name -ne 'powershell' } | ForEach-Object { "$($_.Name)=$($_.Muted)" } | Sort-Object)
"your other audio apps now: $($othersBefore -join ', ')"
# The setting and the tray are the user's: both come back, whatever happens below.
$settingBefore = if ("$(Get-Content -Raw -LiteralPath (Join-Path $state 'mute-test-sounds') -ErrorAction SilentlyContinue)".Trim() -eq 'off') { 'off' } else { 'on' }
try {
    Set-Muting on

    "--- 1. a hidden run stays silent"
    $tone = Start-Tone 'hidden' 25 'a'
    Start-Sleep -Milliseconds 600
    $watch = Watch-Mute $tone.Id 2000
    Report 'hidden tone is muted' ($watch.Samples -gt 0 -and $watch.MutedSamples -eq $watch.Samples) "$($watch.MutedSamples) of $($watch.Samples) samples"
    Stop-Tone $tone
    Start-Sleep -Seconds 1
    Report 'the program stays listed after the run, as Windows would start it muted' ((Read-MutedList) -like '*powershell.exe') (Read-MutedList)

    "--- 2. the same program on your real screen has its sound (you may hear a beep)"
    $tone = Start-Tone 'visible' 25 'b'
    $clock = [Diagnostics.Stopwatch]::StartNew()
    while ($clock.ElapsedMilliseconds -lt 3000 -and @([Sessions]::All() | Where-Object { $_.Pid -eq $tone.Id -and $_.Muted }).Count) { Start-Sleep -Milliseconds 10 }
    "  your launch got its sound after at most $($clock.ElapsedMilliseconds) ms"
    Report 'your launch gets its sound quickly' ($clock.ElapsedMilliseconds -lt 500) "$($clock.ElapsedMilliseconds) ms"
    $watch = Watch-Mute $tone.Id 1200
    Report 'and it stays unmuted' ($watch.Samples -gt 0 -and $watch.MutedSamples -eq 0) "$($watch.MutedSamples) of $($watch.Samples) samples muted"
    Report 'the program leaves the list' (-not (Read-MutedList))
    Stop-Tone $tone
    Start-Sleep -Seconds 1
    "--- 2b. your copy runs while a test of the same program ends; your next launch still gets its sound"
    $yours = Start-Tone 'visible' 25 'b2'
    Start-Sleep -Milliseconds 500
    $tone = Start-Tone 'hidden' 25 'b3'
    Start-Sleep -Milliseconds 800
    Report 'the test is muted beside your copy' (@([Sessions]::All() | Where-Object { $_.Pid -eq $tone.Id -and $_.Muted }).Count -eq 1)
    Report 'your running copy is not' (@([Sessions]::All() | Where-Object { $_.Pid -eq $yours.Id -and -not $_.Muted }).Count -eq 1)
    Stop-Tone $tone
    Start-Sleep -Milliseconds 800
    Report 'your running copy did not clear the list' ((Read-MutedList) -like '*powershell.exe')
    Stop-Tone $yours
    Start-Sleep -Milliseconds 800
    $tone = Start-Tone 'visible' 25 'b4'
    Start-Sleep -Milliseconds 500
    Report 'your next launch has its sound' (@([Sessions]::All() | Where-Object { $_.Pid -eq $tone.Id -and -not $_.Muted }).Count -eq 1)
    Stop-Tone $tone
    Start-Sleep -Seconds 1

    "--- 3. the option off: a hidden run can be heard (you may hear a beep)"
    Set-Muting off
    $tone = Start-Tone 'hidden' 25 'c'
    Start-Sleep -Milliseconds 600
    $watch = Watch-Mute $tone.Id 1200
    Report 'with the option off the hidden tone is not muted' ($watch.Samples -gt 0 -and $watch.MutedSamples -eq 0) "$($watch.MutedSamples) of $($watch.Samples) samples muted"
    Stop-Tone $tone
    Set-Muting on
    Start-Sleep -Seconds 1

    "--- 4. the trap: the guard dies mid-mute, then you launch the same program"
    $tone = Start-Tone 'hidden' 30 'd'
    Start-Sleep -Milliseconds 800
    $watch = Watch-Mute $tone.Id 500
    Report 'muted before the crash' ($watch.Samples -gt 0 -and $watch.Last -eq $true)
    $tray = @(Get-Process quietdesk-tray -ErrorAction SilentlyContinue)
    foreach ($item in $tray) { Stop-Process -Id $item.Id -Force }
    Start-Sleep -Milliseconds 300
    Stop-Tone $tone
    Start-Sleep -Seconds 1
    $listed = Read-MutedList
    Report 'the program is on the repair list' ($listed -like '*powershell.exe') $listed
    $tone = Start-Tone 'visible' 25 'e'
    $withoutGuard = Watch-Mute $tone.Id 500
    "  with no guard running, Windows started it muted: $($withoutGuard.Last)"
    & (Join-Path $bin 'quietdesk.exe') start-tray
    Start-Sleep -Milliseconds 1200
    $watch = Watch-Mute $tone.Id 800
    Report 'the guard gives it its sound back within a second' ($watch.Samples -gt 0 -and $watch.Last -eq $false) "last muted=$($watch.Last)"
    Stop-Tone $tone
    Start-Sleep -Seconds 1
    Report 'the repair list is empty again' (-not (Read-MutedList)) (Read-MutedList)

    "--- 5. a program with no window stays silent too"
    foreach ($case in @(
            @{ Name = '64-bit, hidden'; Word = 'hidden'; Script = $quietScript; Shell = $ps51; Tag = 'q1' },
            @{ Name = '32-bit, hidden'; Word = 'hidden'; Script = $quietScript; Shell = $ps51x86; Tag = 'q2' },
            @{ Name = 'started by PowerShell 7, hidden'; Word = 'hidden'; Script = $chainScript; Shell = 'pwsh'; Tag = 'q3' },
            @{ Name = 'on the virtual monitor'; Word = 'monitor'; Script = $quietScript; Shell = $ps51; Tag = 'q4' },
            @{ Name = 'on the virtual monitor, from inside a job as the Bash tool runs'; Word = 'monitor'; Script = $quietScript; Shell = $ps51; Tag = 'q5'; Inside = $true })) {
        $tone = Start-Tone $case.Word 25 $case.Tag $case.Script $case.Shell ([bool]$case['Inside'])
        Start-Sleep -Milliseconds 600
        $watch = Watch-Mute $tone.Id 1500
        Report "$($case.Name): muted" ($watch.Samples -gt 0 -and $watch.MutedSamples -eq $watch.Samples) "$($watch.MutedSamples) of $($watch.Samples) samples"
        Stop-Tone $tone
        Start-Sleep -Seconds 1
    }
    # Back to a clean list: your launch of each program gets its sound back.
    Restore-TestPrograms -Report
    Report 'the list is empty again' (-not (Read-MutedList)) (Read-MutedList)

    "--- 6. new hidden sound is muted before it can be heard"
    # A fresh copy of a tiny tone program each time, so Windows has no mute remembered for it: only the guard can have muted it. The
    # tone notes the moment it starts playing on the system-wide clock; its session is watched as closely as this script can.
    $toneSource = Join-Path $here 'fresh-tone.cs'
    Set-Content -LiteralPath $toneSource -Encoding ascii -Value @'
using System; using System.Diagnostics; using System.IO; using System.Media; using System.Threading;
static class Tone {
    static void Main(string[] args) {
        var player = new SoundPlayer(args[0]);
        player.Load();
        long at = Stopwatch.GetTimestamp();
        player.PlayLooping();
        File.WriteAllText(args[1], Process.GetCurrentProcess().Id + " " + at);
        Thread.Sleep(1500);
        player.Stop();
    }
}
'@
    $freshTone = Join-Path $here 'fresh-tone.exe'
    & "$env:WINDIR\Microsoft.NET\Framework64\v4.0.30319\csc.exe" /nologo /target:exe "/out:$freshTone" $toneSource | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'the fresh tone program did not compile' }
    $frequency = [Diagnostics.Stopwatch]::Frequency
    $delays = @()
    $copies = @()
    foreach ($run in 1..5) {
        $copy = Join-Path $here "fresh-tone-$([guid]::NewGuid().ToString('N')).exe"
        Copy-Item -LiteralPath $freshTone -Destination $copy
        $copies += $copy
        $note = Join-Path $here "fresh-$run.txt"
        $wrapper = Start-Process -FilePath (Join-Path $bin 'quietdesk.exe') -ArgumentList 'hidden', "`"$copy`"", "`"$wav`"", "`"$note`"" -PassThru -WindowStyle Hidden
        $tonePid = $null; $playAt = $null; $seenAt = $null; $mutedAt = $null
        $clock = [Diagnostics.Stopwatch]::StartNew()
        while ($clock.ElapsedMilliseconds -lt 3000 -and -not $mutedAt) {
            if (-not $tonePid) {
                $words = "$(Get-Content -Raw -LiteralPath $note -ErrorAction SilentlyContinue)".Trim() -split ' '
                if ($words.Count -eq 2) { $tonePid = [int]$words[0]; $playAt = [long]$words[1] }
            }
            $now = [Diagnostics.Stopwatch]::GetTimestamp()
            foreach ($session in @([Sessions]::All() | Where-Object { $tonePid -and $_.Pid -eq $tonePid })) {
                if (-not $seenAt) { $seenAt = $now }
                if ($session.Muted) { $mutedAt = $now }
            }
        }
        [void]$wrapper.WaitForExit(5000)
        # From the first look that found its session to the first that found it muted: 0 when it was muted as soon as it was seen.
        $delays += $(if ($seenAt -and $mutedAt) { [math]::Round(($mutedAt - $seenAt) * 1000.0 / $frequency) } else { 'never' })
        "  run ${run}: session seen $(if ($seenAt) { [math]::Round(($seenAt - $playAt) * 1000.0 / $frequency) } else { '-' }) ms after it began to play, muted $(if ($mutedAt) { [math]::Round(($mutedAt - $playAt) * 1000.0 / $frequency) } else { 'never' }) ms after"
    }
    # A tick of 0.2 s let it play for up to that long; announced, it is muted within one look of this script.
    Report 'new hidden sound is muted as soon as it is seen' (@($delays | Where-Object { $_ -isnot [double] -or $_ -gt 20 }).Count -eq 0) "ms from seen to muted: $($delays -join ', ')"
    # Windows remembers each copy as muted, as it would a real test's program; gone, they leave the list when the guard starts.
    Start-Sleep -Milliseconds 500
    Remove-Item -LiteralPath $copies -Force -ErrorAction SilentlyContinue
    Stop-QuietDeskTray
    & (Join-Path $bin 'quietdesk.exe') start-tray
    Start-Sleep -Milliseconds 1500
    Report 'programs that are gone leave the list when the tray starts' (-not (Read-MutedList)) (Read-MutedList)

    "--- 7. your other apps"
    $othersAfter = @([Sessions]::All() | Where-Object { $_.Name -ne 'powershell' } | ForEach-Object { "$($_.Name)=$($_.Muted)" } | Sort-Object)
    $changed = @($othersBefore | Where-Object { $_ -like '*=True' -and $othersAfter -notcontains $_ }) + @($othersAfter | Where-Object { $_ -like '*=True' -and $othersBefore -notcontains $_ })
    Report 'no other app was muted or unmuted' ($changed.Count -eq 0) "$($othersAfter -join ', ')"
}
finally {
    Set-Muting $settingBefore
    if (-not (Get-Process quietdesk-tray -ErrorAction SilentlyContinue)) { & (Join-Path $bin 'quietdesk.exe') start-tray }
    Remove-Item -LiteralPath $here -Recurse -Force -ErrorAction SilentlyContinue
}
"failures: $failures"
exit $failures
