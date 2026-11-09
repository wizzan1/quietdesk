<#
Proves that what runs through `quietdesk monitor` is followed from any shell, against the installed programs and the running keeper: from
outside any job and from inside a job nothing may leave, as Claude Code's Bash tool runs. A program that places its own window ends up
on the monitor, also when the command that started it ended at once. Exit codes carry through, the log stays quiet and no file is left.
When Windows switches the monitor off, as a change of your screens can, it is shown again where it was, with no new plug-in.

Its windows are fully transparent and take no focus and no input, so nobody sees them even where they must not end up. It needs the
monitor already plugged in, so that it never plugs one in itself: run it with the mode on. Its last step switches the monitor off once,
as Windows would, which a full-screen game may notice as a short stutter.
#>
. (Join-Path $PSScriptRoot 'common.ps1')

$quietdesk = Join-Path $bin 'quietdesk.exe'
$ps51 = "$env:WINDIR\System32\WindowsPowerShell\v1.0\powershell.exe"
$problems = Join-Path $state 'problems.log'
$commandFiles = Join-Path $state 'monitor-commands'
# The notes of a command nobody follows. Other sessions' problems are not this check's.
$notFollowed = 'its windows stay where they open|did not answer in time'
$windowMilliseconds = 3000
$failures = 0

function Report([string]$Name, [bool]$Passed, [string]$Detail = '') {
    "[{0}] {1}{2}" -f $(if ($Passed) { 'PASS' } else { 'FAIL' }), $Name, $(if ($Detail) { ": $Detail" } else { '' })
    if (-not $Passed) { $script:failures++ }
}

function Get-CommandFile { return @(Get-ChildItem -LiteralPath $commandFiles -File -ErrorAction SilentlyContinue | ForEach-Object Name) }
function Get-Keeper { return @(Get-CimInstance Win32_Process -Filter "Name = 'quietdesk.exe'" | Where-Object { $_.CommandLine -match 'monitor-keeper' } | ForEach-Object ProcessId) }

# Which displays Windows shows, from Windows itself. Also the code of the step that switches the monitor off, in its own process.
$displayType = @'
using System; using System.Collections.Generic; using System.Linq; using System.Runtime.InteropServices;
public static class QuietDeskDisplays {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] struct Device { public int cb; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string Name; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string String; public int Flags; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string Id; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 128)] public string Key; }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern bool EnumDisplayDevicesW(string device, int index, ref Device info, int flags);
    [StructLayout(LayoutKind.Sequential)] struct Luid { public uint Low; public int High; }
    [StructLayout(LayoutKind.Sequential)] struct Source { public Luid Adapter; public uint Id; public uint ModeIdx; public uint Status; }
    [StructLayout(LayoutKind.Sequential)] struct Target { public Luid Adapter; public uint Id; public uint ModeIdx; public uint Tech; public uint Rotation; public uint Scaling; public uint RateN; public uint RateD; public uint ScanLine; public int Available; public uint Status; }
    [StructLayout(LayoutKind.Sequential)] struct Path { public Source Source; public Target Target; public uint Flags; }
    [StructLayout(LayoutKind.Sequential)] struct Mode { public uint Type; public uint Id; public Luid Adapter; [MarshalAs(UnmanagedType.ByValArray, SizeConst = 48)] public byte[] Data; }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] struct SourceName { public uint Type; public uint Size; public Luid Adapter; public uint Id; [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string Gdi; }
    [DllImport("user32.dll")] static extern int GetDisplayConfigBufferSizes(uint flags, out uint paths, out uint modes);
    [DllImport("user32.dll")] static extern int QueryDisplayConfig(uint flags, ref uint pathCount, [Out] Path[] paths, ref uint modeCount, [Out] Mode[] modes, IntPtr topology);
    [DllImport("user32.dll")] static extern int SetDisplayConfig(uint pathCount, Path[] paths, uint modeCount, Mode[] modes, uint flags);
    [DllImport("user32.dll")] static extern int DisplayConfigGetDeviceInfo(ref SourceName info);
    const uint OnlyActive = 2, Apply = 0x80, UseSupplied = 0x20, AllowChanges = 0x400;
    // The names of the virtual monitors Windows shows now.
    public static List<string> VirtualShown() {
        var names = new List<string>();
        for (int i = 0; ; i++) {
            var d = new Device(); d.cb = Marshal.SizeOf(d);
            if (!EnumDisplayDevicesW(null, i, ref d, 0)) break;
            if ((d.Flags & 1) != 0 && d.String.Contains("Parsec")) names.Add(d.Name);
        }
        return names;
    }
    // Windows' own way of switching displays off, as a change of the user's screens does: the virtual ones go, nothing is saved.
    // 0 when done, else why not.
    public static int SwitchVirtualOff() {
        var virtualNames = VirtualShown();
        if (virtualNames.Count == 0) return -1;
        uint pc, mc;
        int e = GetDisplayConfigBufferSizes(OnlyActive, out pc, out mc); if (e != 0) return e;
        var paths = new Path[pc]; var modes = new Mode[mc];
        e = QueryDisplayConfig(OnlyActive, ref pc, paths, ref mc, modes, IntPtr.Zero); if (e != 0) return e;
        var kept = paths.Take((int)pc).Where(p => {
            var n = new SourceName { Type = 1, Size = (uint)Marshal.SizeOf(typeof(SourceName)), Adapter = p.Source.Adapter, Id = p.Source.Id };
            return DisplayConfigGetDeviceInfo(ref n) != 0 || !virtualNames.Contains(n.Gdi);
        }).ToArray();
        if (kept.Length == (int)pc) return -2;
        return SetDisplayConfig((uint)kept.Length, kept, mc, modes.Take((int)mc).ToArray(), Apply | UseSupplied | AllowChanges);
    }
}
'@
Add-Type -TypeDefinition $displayType

# From `quietdesk status`, which asks the keeper: a file a killed keeper left behind would have this check plug in a monitor itself.
$status = (& $quietdesk status | Out-String)
if ($LASTEXITCODE -ne 0) { throw "quietdesk status failed with exit code $LASTEXITCODE." }
if ($status -notmatch 'Virtual monitor: in use, at (-?\d+),(-?\d+) size (\d+)x(\d+)') { throw 'No virtual monitor is plugged in. Run this with the mode on: it never plugs one in itself.' }
$left = [int]$Matches[1]
$top = [int]$Matches[2]
$right = $left + [int]$Matches[3]
$bottom = $top + [int]$Matches[4]

$here = Join-Path ([IO.Path]::GetTempPath()) "quietdesk-verify-monitor-$PID"
New-Item -ItemType Directory -Force -Path $here | Out-Null
$windowScript = Join-Path $here 'window.ps1'
Set-Content -LiteralPath $windowScript -Encoding ascii -Value @'
param([string]$ResultFile, [int]$Milliseconds)
Add-Type -WarningAction SilentlyContinue -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @"
using System.Drawing; using System.Windows.Forms;
public class SelfPlacedWindow : Form {
    const int Layered = 0x80000, ClickThrough = 0x20, NoActivate = 0x8000000, ToolWindow = 0x80;
    protected override bool ShowWithoutActivation { get { return true; } }
    protected override CreateParams CreateParams { get { CreateParams p = base.CreateParams; p.ExStyle |= Layered | ClickThrough | NoActivate | ToolWindow; return p; } }
    public SelfPlacedWindow() { FormBorderStyle = FormBorderStyle.None; StartPosition = FormStartPosition.Manual; Location = new Point(100, 100); Size = new Size(300, 200); ShowInTaskbar = false; Opacity = 0; }
}
"@
Add-Type -AssemblyName System.Windows.Forms
$window = New-Object SelfPlacedWindow
$timer = New-Object System.Windows.Forms.Timer
$timer.Interval = $Milliseconds
$timer.Add_Tick({ $timer.Stop(); "$($window.Left) $($window.Top)" | Set-Content -LiteralPath $ResultFile; $window.Close() })
$timer.Start()
[System.Windows.Forms.Application]::Run($window)
'@

function Get-WindowArguments([string]$ResultFile) { return "-NoProfile -ExecutionPolicy Bypass -File `"$windowScript`" `"$ResultFile`" $windowMilliseconds" }

function Test-EndsOnMonitor([string]$Name, [string]$ResultFile) {
    # Read until it holds both numbers: the window may be in the middle of writing them.
    $deadline = (Get-Date).AddSeconds(30)
    do {
        $place = "$(Get-Content -Raw -LiteralPath $ResultFile -ErrorAction SilentlyContinue)" -match '^(-?\d+) (-?\d+)\s*$'
        if (-not $place) { Start-Sleep -Milliseconds 100 }
    } while (-not $place -and (Get-Date) -lt $deadline)
    if (-not $place) {
        Report $Name $false 'the window never reported where it was'
        return
    }
    $x = [int]$Matches[1]
    $y = [int]$Matches[2]
    Report $Name ($x -ge $left -and $x -lt $right -and $y -ge $top -and $y -lt $bottom) "it placed itself at 100,100 and ended at $x,$y; the monitor starts at $left,$top"
}

$linesBefore = @(Get-Content -LiteralPath $problems -ErrorAction SilentlyContinue).Count
$filesBefore = @(Get-CommandFile)
try {
    '--- from outside any job'
    $result = Join-Path $here 'outside.txt'
    & $quietdesk monitor $ps51 -NoProfile -ExecutionPolicy Bypass -File $windowScript $result $windowMilliseconds
    Report 'the command ended well' ($LASTEXITCODE -eq 0) "exit $LASTEXITCODE"
    Test-EndsOnMonitor 'a window that places itself ends on the monitor' $result

    '--- from inside a job nothing may leave, as the Bash tool runs'
    $result = Join-Path $here 'inside.txt'
    $run = Start-InsideJob "`"$quietdesk`" monitor `"$ps51`" $(Get-WindowArguments $result)"
    Report 'the command ended well' ($run.WaitForExit(30000) -and $run.ExitCode -eq 0) "exit $($run.ExitCode)"
    Test-EndsOnMonitor 'a window that places itself ends on the monitor' $result

    '--- from inside that job, a launcher that ends at once and leaves the program running'
    $result = Join-Path $here 'launcher.txt'
    $run = Start-InsideJob "`"$quietdesk`" monitor cmd.exe /c start `"`" /b `"$ps51`" $(Get-WindowArguments $result)"
    $endedInTime = $run.WaitForExit($windowMilliseconds)
    Report 'the command ended before its window' ($endedInTime -and -not (Test-Path -LiteralPath $result)) "exit $($run.ExitCode)"
    Test-EndsOnMonitor 'the window it left behind ends on the monitor' $result

    '--- from inside that job, an exit code'
    $run = Start-InsideJob "`"$quietdesk`" monitor cmd.exe /c `"exit 7`""
    Report 'the exit code carries through' ($run.WaitForExit(30000) -and $run.ExitCode -eq 7) "exit $($run.ExitCode)"

    '--- what a command costs now, against one with no monitor (medians of 15; no limit: read it)'
    foreach ($word in 'visible', 'monitor') {
        $times = @(1..15 | ForEach-Object {
                (Measure-Command { & $quietdesk $word cmd.exe /c 'exit 0' }).TotalMilliseconds
                if ($LASTEXITCODE -ne 0) { throw "quietdesk $word failed with exit code $LASTEXITCODE." }
            } | Sort-Object)
        "  quietdesk $word`: {0:N0} ms (from {1:N0} to {2:N0})" -f $times[7], $times[0], $times[14]
    }

    '--- Windows switches the monitor off, as a change of your screens can: it must be shown again, with no new plug-in'
    # The plug-in count below comes from the driver's own log entries: first prove they can be read at all, or 0 would prove nothing.
    # The running keeper plugged the monitor in after it started: an entry from before then would be another monitor's.
    $keeperBefore = @(Get-Keeper)
    $keeperStarted = if ($keeperBefore.Count -eq 1) { (Get-Process -Id $keeperBefore[0]).StartTime.AddSeconds(-1) } else { Get-Date }
    $earlierPlugIns = @(Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Parsec Virtual Display Driver'; Id = 6; StartTime = $keeperStarted } -ErrorAction SilentlyContinue).Count
    Report "the driver's plug-in log can be read" ($earlierPlugIns -ge 1) $(if ($earlierPlugIns -ge 1) { 'the plug-in of the monitor that is plugged in now is there' } else { "no plug-in since the keeper started ($keeperStarted) was found in the System log" })
    $since = Get-Date
    $switchOff = Join-Path $here 'switch-off.ps1'
    Set-Content -LiteralPath $switchOff -Encoding ascii -Value "Add-Type -TypeDefinition @'`n$displayType`n'@`nexit [QuietDeskDisplays]::SwitchVirtualOff()"
    # Only the input desktop may change what Windows shows.
    & $quietdesk visible pwsh -NoProfile -File $switchOff
    $switched = $LASTEXITCODE
    Report 'Windows switched the monitor off' ($switched -eq 0) "result $switched"
    $offAt = [Diagnostics.Stopwatch]::StartNew()
    do {
        $shown = @([QuietDeskDisplays]::VirtualShown()).Count -eq 1
        if (-not $shown) { Start-Sleep -Milliseconds 100 }
    } while (-not $shown -and $offAt.Elapsed.TotalSeconds -lt 10)
    Report 'the monitor is shown again' $shown "after $($offAt.ElapsedMilliseconds) ms"
    $keeperAfter = @(Get-Keeper)
    Report 'by the same keeper' ($keeperBefore.Count -eq 1 -and "$keeperAfter" -eq "$keeperBefore") "before [$keeperBefore], after [$keeperAfter]"
    # The driver logs every plug-in. None means no connect sound and no freeze.
    $plugIns = @(Get-WinEvent -FilterHashtable @{ LogName = 'System'; ProviderName = 'Parsec Virtual Display Driver'; Id = 6; StartTime = $since } -ErrorAction SilentlyContinue).Count
    Report 'with no new plug-in' ($plugIns -eq 0) "$plugIns plug-ins"
    $status = (& $quietdesk status | Out-String)
    Report 'where it was' ($status -match "Virtual monitor: in use, at $left,$top size $($right - $left)x$($bottom - $top)") (($status -split "`n" | Select-String 'Virtual monitor').Line)

    '--- what it leaves behind'
    $new = @(Get-Content -LiteralPath $problems -ErrorAction SilentlyContinue | Select-Object -Skip $linesBefore | Where-Object { $_ -match $notFollowed })
    Report 'no command went unfollowed' ($new.Count -eq 0) ($new -join ' | ')
    $deadline = (Get-Date).AddSeconds(5)
    do {
        $stillThere = @(Get-CommandFile | Where-Object { $filesBefore -notcontains $_ })
        if ($stillThere.Count -eq 0) { break }
        Start-Sleep -Milliseconds 100
    } while ((Get-Date) -lt $deadline)
    Report 'no command file is left behind' ($stillThere.Count -eq 0) ($stillThere -join ', ')
}
finally {
    Remove-Item -LiteralPath $here -Recurse -Force -ErrorAction SilentlyContinue
}
"failures: $failures"
exit $failures
