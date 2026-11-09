# What the tools share: where things live, the three edits that must be done carefully, and how a check runs a command as the Bash tool does.
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$claudeDir = Join-Path $HOME '.claude'
# One switch for every agent on the PC, so not in an agent's own folder.
$state = Join-Path $env:LOCALAPPDATA 'QuietDesk'
$bin = Join-Path $state 'bin'
$settingsPath = Join-Path $claudeDir 'settings.json'
$claudeMd = Join-Path $claudeDir 'CLAUDE.md'
$skillDir = Join-Path $claudeDir 'skills\quietdesk'
$ruleStart = '<!-- quietdesk:start -->'
$ruleEnd = '<!-- quietdesk:end -->'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$runValue = 'QuietDesk'
$prefixValue = Join-Path $bin 'quietdesk-bash.exe'

# Puts the programs' folder first on the user PATH, or removes it. Kept as an expandable value: the usual .NET call would
# freeze %VARIABLES% in it. Windows is told, so programs started from now on see the change.
function Set-QuietDeskOnUserPath([bool]$Present) {
    $environment = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
    try {
        $current = [string]$environment.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $others = @($current -split ';' | Where-Object { $_ -and $_.TrimEnd('\') -ine $bin })
        $entries = if ($Present) { @($bin) + $others } else { $others }
        $environment.SetValue('Path', $entries -join ';', [Microsoft.Win32.RegistryValueKind]::ExpandString)
    }
    finally { $environment.Close() }

    if (-not ('QuietDesk.Broadcast' -as [type])) {
        Add-Type -Namespace QuietDesk -Name Broadcast -MemberDefinition @'
[DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
public static extern IntPtr SendMessageTimeoutW(IntPtr hWnd, uint msg, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);
'@
    }
    $HWND_BROADCAST = [IntPtr]0xFFFF
    $WM_SETTINGCHANGE = 0x1A
    $SMTO_ABORTIFHUNG = 2
    $ignored = [UIntPtr]::Zero
    if ([QuietDesk.Broadcast]::SendMessageTimeoutW($HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, 'Environment', $SMTO_ABORTIFHUNG, 3000, [ref]$ignored) -eq [IntPtr]::Zero) {
        Write-Warning 'Windows did not confirm the PATH change: restart or sign in again if the Claude app does not see it.'
    }
}

# Every session depends on this file: it is proven to read back, and replaced in one step.
function Write-ClaudeSettings($Settings) {
    $json = $Settings | ConvertTo-Json -Depth 32
    [void]($json | ConvertFrom-Json)
    $temporary = "$settingsPath.quietdesk-tmp"
    [IO.File]::WriteAllText($temporary, $json, [Text.UTF8Encoding]::new($false))
    Move-Item -LiteralPath $temporary -Destination $settingsPath -Force
}

# Starts a command line in a new job nothing may leave, as Claude Code's Bash tool runs, with a console nobody sees. Waits like a Process.
function Start-InsideJob([string]$CommandLine) {
    if (-not ('QuietDesk.InsideJob' -as [type])) {
        Add-Type -TypeDefinition @'
using System; using System.ComponentModel; using System.Runtime.InteropServices; using System.Text;
namespace QuietDesk {
    public sealed class InsideJob {
        [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] struct StartupInfo {
            public int Size; public string Reserved, Desktop, Title; public int X, Y, Width, Height, Columns, Rows, Fill, Flags;
            public short Show, Reserved2; public IntPtr Reserved3, Input, Output, Error;
        }
        [StructLayout(LayoutKind.Sequential)] struct ProcessInformation { public IntPtr Process, Thread; public int ProcessId, ThreadId; }
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern IntPtr CreateJobObjectW(IntPtr attributes, string name);
        [DllImport("kernel32.dll", SetLastError = true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)] static extern bool CreateProcessW(string application, StringBuilder line, IntPtr a, IntPtr b, bool inherit, uint flags, IntPtr environment, string folder, ref StartupInfo startup, out ProcessInformation process);
        [DllImport("kernel32.dll")] static extern uint ResumeThread(IntPtr thread);
        [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr process, uint code);
        [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
        [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
        [DllImport("kernel32.dll")] static extern bool GetExitCodeProcess(IntPtr process, out uint code);
        const uint Suspended = 4, NoWindow = 0x08000000, Finished = 0;
        readonly IntPtr process;
        public InsideJob(string commandLine) {
            IntPtr job = CreateJobObjectW(IntPtr.Zero, null);
            if (job == IntPtr.Zero) throw new Win32Exception();
            var startup = new StartupInfo { Size = Marshal.SizeOf(typeof(StartupInfo)) };
            ProcessInformation started;
            if (!CreateProcessW(null, new StringBuilder(commandLine), IntPtr.Zero, IntPtr.Zero, false, Suspended | NoWindow, IntPtr.Zero, null, ref startup, out started)) throw new Win32Exception();
            if (!AssignProcessToJobObject(job, started.Process)) {
                var refused = new Win32Exception();
                TerminateProcess(started.Process, 1);
                throw refused;
            }
            // The job stays for as long as a process is in it, and has no limit that closing it would trigger.
            CloseHandle(job);
            ResumeThread(started.Thread);
            CloseHandle(started.Thread);
            process = started.Process;
        }
        public bool WaitForExit(int milliseconds) { return WaitForSingleObject(process, (uint)milliseconds) == Finished; }
        public bool HasExited { get { return WaitForExit(0); } }
        public uint ExitCode { get { uint code; GetExitCodeProcess(process, out code); return code; } }
    }
}
'@
    }
    return [QuietDesk.InsideJob]::new($CommandLine)
}

# Only a tray running from this install. It is asked to close, as its own window would be, so its sound guard gives back every mute
# it holds; a kill would leave them for Windows to remember.
function Stop-QuietDeskTray {
    $running = @(Get-Process -Name 'quietdesk-tray' -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path -like "$bin\*" })
    if (-not $running.Count) { return }
    & (Join-Path $bin 'quietdesk.exe') quit-tray
    if ($LASTEXITCODE -ne 0) { Write-Warning "quietdesk quit-tray failed with exit code $LASTEXITCODE." }
    foreach ($tray in $running) {
        if (-not $tray.WaitForExit(5000)) {
            Write-Warning 'The tray did not close within 5 s, so it was stopped: a test sound it had muted may start muted next time.'
            Stop-Process -Id $tray.Id -Force
            [void]$tray.WaitForExit(3000)
        }
    }
}
