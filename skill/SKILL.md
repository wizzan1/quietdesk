---
name: quietdesk
description: QuietDesk for Claude Code on Windows, where what test runs open stays on a hidden desktop off the user's screen. This skill should be used when the user says "quiet desk", "/quietdesk", "do not disturb", "hide the windows" or "show the windows"; when testing a game or app that opens windows while the mode may be on; when a window, screenshot or app behaves oddly or seems missing; when seeing, reading or operating a hidden window; or when changing QuietDesk itself.
---

# QuietDesk

While the mode is on, what Claude Code's commands open runs on a hidden Windows desktop, on the same PC, with the same files, network and GPU, at full speed. Nothing there reaches the screen, takes the focus, or gets the mouse and keyboard. It applies to every project and session.

**The rule: the mode must never reduce what gets tested.** Test exactly as with it off. Never skip, shorten or weaken a check because of it. A test that needs the real screen wins over not disturbing, and the report says so.

## The switch

Run `quietdesk on`, `quietdesk off`, `quietdesk toggle` or `quietdesk status` when the user asks. To the arguments of `/quietdesk`, apply the same words. The user can also click the tray icon: a red disc with a white bar means on, a grey ring means off.

- A flip reaches every connected session on its next command. Windows already running stay where they are: Windows cannot move a window between desktops.
- The first shim of a run decides its place, and everything below stays there, so a whole test run stays together.
- `quietdesk status` shows whether this session is connected. A session started before the install needs the Claude app restarted.
- **Turning it on or off plugs the virtual monitor in or out:** the connect sound and about a second's freeze, once. Tell the user before flipping it for them while they may be playing.

## Run one command elsewhere

Use the first that works; each step up is more noticeable.

1. Hidden: the default while the mode is on. `quietdesk hidden <command...>` does it whatever the mode.
2. `quietdesk monitor <command...>` runs it on a virtual monitor, out of sight, but to Windows a real display. Use it when hidden behaves differently. The keeper follows everything the command starts, moves its windows there and gives the focus back. A window its program places on the real screen leaves it within milliseconds, but only once the program is free: one busy as its window opens, as a game is while it loads, stays visible until then. **With the mode off it plugs a monitor in and out for that command, which freezes the user's screens for about a second each time: never do that during their game.** Without the driver, or with the mode off, a command that gets no monitor still runs, on the real screen, and says so. With the driver and the mode on it does not: it ends with 127 and the reason, also in `quietdesk status`. Rerun it with `quietdesk visible` only if the real screen is acceptable, and say so.
3. `quietdesk visible <command...>` runs it on the real screen. Use it for what needs the keyboard focus, and for anything left open for the user to try.

- `quietdesk desktop` prints where this shell runs: `QuietDesk` is hidden, `Default` is the real screen.

## Programs the user always wants on their screen

Each user keeps their own list: `quietdesk exclude add <program>`, `quietdesk exclude remove <program>`, `quietdesk exclude list`. It starts empty and nothing is built in. It is matched by program file name: `blender`, `Blender.exe` and a full path all mean `blender.exe`. It lives in `%LOCALAPPDATA%\QuietDesk\always-on-your-screen`, and `quietdesk status` and the tray menu show it. Change it only when the user asks: what may disturb them is their choice.

- While the mode is on, a command (one Bash or PowerShell tool call, a hook, an MCP server's start) whose text names a listed program, as a word or as the end of a path, runs on the real screen as a whole, with no prefix: `blender scene.blend`, `& "C:\Program Files\Blender Foundation\Blender 5.2\blender.exe"`, `Start-Process blender`. Its windows open normally on the user's screen.
- Everything else that command starts is on the real screen too. Start the listed program in a command of its own, and keep tests out of it. A mere mention also counts (`Get-Process blender`); that loses nothing but quiet.
- The list is read as a command starts, so a change counts from the next command, never from the one that makes it.
- A command that does not name the program, such as a script that starts it, runs where the mode puts it: hidden. Name the program in the command, or use `quietdesk visible`.
- A window that belongs on the user's screen, of a listed program or started through `quietdesk visible`, is moved from the virtual monitor to the main screen, maximized if it was, whenever it lands there, for as long as it lives. That covers opening there, being put back there by Windows when the monitor returns, and a program put on the list while its window is there. One that opens minimized is restored. Any other program's windows stay where they opened. Like every move, it waits until the program is free: Blender, while loading, was on the main screen after about 2.5 s.
- If the user cannot find such a window, look before you move anything: `GetWindowRect` from the real desktop (`quietdesk visible`) against the monitor's place in `quietdesk status`.

## See, read and talk

Prefer the app's own channel (a control pipe, a console, a debug port, its own screenshots), then these. None uses the real mouse or keyboard. `<window>` is `0xHANDLE`, a process id, or part of the title, class or program name of a window out of sight; a name never matches the user's windows on the real screen.

| Do | Command |
| --- | --- |
| List windows out of sight, hidden or on the monitor | `quietdesk windows` |
| Save a picture (exit 3 means it came out black) | `quietdesk shot <window> <file.png>` |
| List child controls: handle, class, text | `quietdesk controls <window>` |
| Set a control's text | `quietdesk settext <0xCONTROL> <text>` |
| Press a button | `quietdesk press <0xCONTROL>` |
| Send characters | `quietdesk keys <window> <text>` |
| Click at client coordinates | `quietdesk click <window> <x> <y>` |
| Ask it to close | `quietdesk close <window>` |

Chrome and Chromium apps: give each its own `--user-data-dir`, then `--remote-debugging-port=<port>`, and read and click through `Runtime.evaluate`. Headless also works.

## When hidden differs

| Symptom | Cause | Do |
| --- | --- | --- |
| A DirectX app stops drawing or quits | Windows reports its window as not visible from the first frame. Frames still reach a capture if the app keeps drawing | `quietdesk monitor`, or fix the app to keep drawing |
| `quietdesk shot` exits 3 on an OpenGL window, or the tray's preview is black | OpenGL windows capture black when hidden. The game is running normally, and its own screenshots are real | The game's own screenshots, or `quietdesk monitor` |
| A check measures what the display shows | Nothing reaches a display | `quietdesk monitor` |
| A check needs the keyboard focus | The monitor gives the focus back, so a focus check would pass there without proving anything | `quietdesk visible` |
| A check of display timing reports a regression | A full-screen program, usually the user's game, holds the main screen. See the traps | Rerun it when the game is closed, and report it as not run |
| A program asks for a resolution | Windows refuses a hidden program a display mode change (`ChangeDisplaySettingsEx` returns -1, 0 on the real desktop), so it can never change the user's. A full-screen game still runs at the size it asked for | A test of a real mode change needs `quietdesk visible` |
| Edge exits at once with code 0, also headless | Edge refuses another desktop. Apps built on it may too (untested) | `quietdesk monitor`, or Chrome |
| A command seems hung | Possibly a dialog waiting for an answer, such as a sign-in | `quietdesk windows`, `quietdesk shot`, then answer it or `quietdesk close` |
| UI Automation lists names but no actions | Seen on the hidden desktop | `quietdesk controls`, `quietdesk press`, `quietdesk settext` |
| A program exits with 0xC0000135 | A DLL is missing. On the real screen this would be Windows' error box; hidden it just exits, by design (see the traps) | Fix the DLL |
| Motion looks choppy in the tray's preview | It refreshes every 700 ms by design, whatever the app's frame rate | Read the app's own frame rate |

## What was measured

Ryzen 9 7950X, RTX 3060 Ti, 2560x1440 at 165 Hz plus 1920x1080 at 60 Hz, November 2026. **With no game running, hidden matched the real screen in every number.**

| Question | Result |
| --- | --- |
| Timers | The same: `Sleep(1)` with `timeBeginPeriod(1)` is a median of 1.50 ms hidden and on the real screen |
| GPU | The real one: `GL_RENDERER` NVIDIA, `hz:165` |
| A 33-step game test suite (Reaction, an ioquake3 game) from a hidden shell | All passed, with its display check on the virtual monitor and its focus check on the real screen; 16.8 minutes against 16.9 |
| Frame rate, the same run in both places | 165 fps with vsync and 330 without, at 720p and 1440p, with the same p99 (6.3 and 5.0 ms), hidden and on the real screen |
| Size | 2560x1440 and full screen both run hidden, with the right size and a real picture in the game's own screenshot |
| DWM's clock with the monitor plugged in | 165 Hz with the screen free, so display checks still run |
| Several at once | Three game clients ran together hidden and all finished |
| A command's extra cost | About 30 ms through the Bash shim (25 ms with the mode off), and about 40 ms through `quietdesk hidden` |
| `git` and `gh` from a hidden shell | Worked with no dialog, on the user's stored sign-in |
| Turning it on or off, no game running | The command takes 30 to 50 ms. The monitor is there within a second, and gone within a second of `quietdesk off` (up to 20 s when it was only just turned on, as a new keeper waits for its first command). In a full-screen game, expect about a second's freeze with the connect sound |
| With the mode off | No keeper and no monitor, the user's console setting back, the tray at 0.4 % of one core |
| The tray | 0.3 to 0.4 % of one core with the sound guard idle, 15 to 18 MB |
| The monitor staying plugged | With the mode on it stayed through 25 s idle, a command on it and a hidden game, with no change of screens in 25 ms sampling. The keeper costs 0.3 % of one core and 10 MB |
| `quietdesk monitor` from any shell | A window that places itself ended on the monitor from outside a job, from inside a job nothing may leave, and after a launcher that ended at once. A windowless tone there stayed muted. A command takes a median of 70 ms against 57 with no monitor, at most 211 |
| `quietdesk monitor`, Parsec driver 0.41 | DX11 presented normally, an OpenGL game captured from outside and its display check passed, Edge ran. A window that placed itself on the real screen was off it in about 1 ms (at most 19), the focus not lost; one whose program was busy for 1.5 s as it opened stayed for those 1.5 s |
| A hidden test's sound | Muted as soon as it opened its sound, in 10 of 10 runs (fresh programs, so no mute Windows remembered). Looking every 0.2 s, it played for 25 to 190 ms |

**What slows a test run is sharing the GPU with the user's game, not the mode.** With a full-screen game using about 78 % of the GPU, an OpenGL game at 720p drew a mean of 60 fps with vsync and 93 without when hidden, and 42 and 39 on the virtual monitor. Game test steps took about a third longer, some more than twice as long. Hidden was the faster of the two out of sight, because nothing has to be composed.

## What can still reach the user

Tested and kept out: a missing DLL's error box, a real crash, a message box, a hidden program's resolution request.

Still reaches them:

- **A window on `quietdesk monitor` whose program is busy as it opens**, until the program is free (see the traps).
- **GPU load:** cap the frame rate of test runs. The user may be playing.
- **Sound on the monitor from a program not yet known as a test's**, such as one started outside the command: until the guard's next look, within 0.2 seconds.

Not tested, and expected to reach the real screen, because another process does the opening: an elevation prompt, a program that hands over to an instance already on the real screen, and what the shell or COM opens on request (`start <url>`, a folder, a Store app). Tools the Claude app starts itself, such as its preview browser, do not pass through the shims.

## Sound

What runs out of sight is muted with Windows' per-program mute, window or no window, 32-bit or 64-bit, and the user's own game and apps never are: `quietdesk mute on|off`, or the tray menu. The guard runs in the tray app, and `quietdesk status` says if it is not running. Windows tells it the moment any program opens a sound session, which is before it plays, and it also looks every 0.2 seconds for what no announcement covers. Programs still play through the same code, so a test of sound logic is unaffected; a test that must be heard needs `quietdesk mute off` or `quietdesk visible`.

## How it works

Two thin programs sit below Claude Code's permission checks, so commands are not rewritten and those checks still see the real command. Never hook or rewrite the command text instead: permission rules check what a hook returns.

| Program | Where | Catches |
| --- | --- | --- |
| `pwsh.exe` shim | First on the user PATH. Claude Code finds PowerShell by name | PowerShell tool. Acts only when `CLAUDECODE` is set, so the user's own terminals are untouched |
| `quietdesk-bash.exe` | `CLAUDE_CODE_SHELL_PREFIX` in `~/.claude/settings.json` | Bash tool, hooks and MCP servers |

Claude Code starts the Bash shim two ways (read from Claude Code 2.1.317 and captured). For the Bash tool and hooks, Git Bash runs `bash -c -l "'<prefix>' '<script>'"`; the shim reads the script from that bash's own command line and the environment Claude Code gave it from that process (`initial_environment_of`), and hands both to the same bash, the script quoted so Git Bash reads back exactly what it was given (`src/shell.rs`). For an MCP server, Claude Code starts the shim itself with the command and arguments as one line of shell words; the shim splits them and starts the program as Claude Code's own starter, cross-spawn, would with no prefix: found by PATHEXT, a batch file such as `npx.cmd` through `cmd.exe`, a script through the interpreter on its `#!` line. Hooks written with `"args"` bypass the prefix, and are not hidden. An MCP server takes its place when it starts, so a flip of the mode reaches it only when it is reconnected (`/mcp`).

`quietdesk monitor` adds a keeper process (`src/monitor.rs`): it owns the Parsec driver's display, which exists only while it is pinged, and tracks the commands' processes. It gives the monitor the main screen's size and the nearest rate the driver has, in the corner of the main screen so the mouse does not wander onto it. It stays while the mode is on, and otherwise unplugs a few seconds after the last process ends, even if a game outlives the command. It also moves a window that belongs on the user's screen (a listed program's, or one started through `quietdesk visible`) off the monitor to the main screen, and restores one that opens minimized. Each command gets a job of its own and a file in `%LOCALAPPDATA%\QuietDesk\monitor-commands`. It starts only once the keeper has answered that it holds the job, and the keeper removes the file when the job is empty.

`quietdesk on` also sets new consoles to the classic console, because a new console from a hidden command otherwise hands over to Windows Terminal, which opens on the real screen. `quietdesk off` gives the user's setting back. The setting lives in `HKCU\Console\%%Startup`, and a packaged app's processes can see a copy of it of their own, so the first shim of every command also puts its own copy right for the mode, with the user's choice kept in a `QuietDeskBefore` subkey of that same copy. `quietdesk status` says where a new console from that shell opens.

The sound guard asks each audio program's own process record which desktop it belongs to, because a program with no window leaves no other trace. Shims set `SEM_FAILCRITICALERRORS`, which everything below them inherits.

## Changing it

Rust, because the shims run on every command. State and programs live in `%LOCALAPPDATA%\QuietDesk`; problems the programs could not report go to `problems.log` there, and `quietdesk status` shows the last one.

1. Run `cargo test` for the programs' paths.
2. Run `pwsh -File tools\install.ps1` to install. Running sessions keep working, but it restarts a running keeper. With the mode on that replugs the monitor, so not during the user's game, and not while a session uses `quietdesk monitor`.
3. Run `pwsh -File tools\verify-monitor.ps1` with the mode on for `quietdesk monitor` from any shell. Nobody can see its windows, so it can run during a game.
4. Run `pwsh -File tools\verify-sound.ps1` for sound. It can be heard, opens small windows and restarts the tray. With the mode off it also plugs the monitor in, so not during the user's game.
5. Run `pwsh -File tools\verify.ps1` to drive a real, throwaway session through the installed mode, and read its results. From a hidden shell it reruns itself on the real desktop, because that is what it proves. It leaves the user's own mode as it was.

Traps that already happened or would break every session:

- **Two command-line parsers meet in the Bash shim.** A Windows program reads its command line by the C runtime's rules; Git Bash, started by a Windows program, reads it by Cygwin's, which turn `\\` into `\` inside double quotes. Handing bash the raw command line halved every doubled backslash a second time (a heredoc that wrote `a\\b` wrote `a\b`), and Git Bash's own conversion of variables for a Windows program left `$PWD` as `C:/...` and `$OLDPWD` likewise; with the mode on or off. Only a comparison run the way Claude Code runs it found it: the old test passed its script from Rust, with single backslashes. A script also lost what Git Bash rewrites for a Windows program: `/usr/bin/env` at its start became `C:/Program Files/Git/usr/bin/env`, and `PATH` named `/bin` as `/usr/bin`. So the script comes from the parent bash's command line and the environment from the parent bash's process. `bash_scripts_reach_bash_exactly_as_git_bash_hands_them_over` starts Git Bash from a Windows program with `-c -l`, as Claude Code does, and compares every case, the whole environment and the shell options, with `PWD` given and not, with no shim at all.
- **Git Bash cuts a long command.** Started by a Windows program, it drops what follows about the first 8,190 characters of its command, and Claude Code hands every Bash command over that way: a longer one fails with `unexpected EOF while looking for matching`, with or without QuietDesk. The prefix takes about 80 of those characters, and 4 more per single quote. Put a long script in a file first and run that.
- **Windows gives every program the real Program Files folder,** whatever its parent set: a test cannot point `ProgramFiles` elsewhere. The PowerShell shim looks where Claude Code looks when PATH has no PowerShell 7 after it (Program Files, the Store's alias, `.dotnet\tools`), never at Windows PowerShell 5.1: having found a program named pwsh, Claude Code tells the model it has PowerShell 7.
- **An MCP server is not a script.** Claude Code hands the prefix an MCP server's command as shell words and starts it itself, not from Git Bash. Run through bash, Git Bash turned `/c` into `C:/`, so `cmd /c npx ...`, the form Claude Code documents for Windows, started an interactive `cmd` and the server never came up. The shim tells the two apart by its parent process: Git Bash means a script.
- **A command nothing follows does not start.** Without the keeper's answer a command on the monitor would open on the user's screen, unmuted and free to take the focus. It gets one more try with a new keeper, then fails and says so.
- **Two shims on PATH start each other without end** (6252 processes once). `shim_dirs_including` prevents it; keep that test.
- **Never delete the installed programs while sessions run.** They remember the shim's path. `uninstall.ps1` keeps the files until the app restarts.
- **Touching a window from another desktop fails:** its child list comes back empty and messages fail with invalid handle. Go through `quietdesk::on_desktop`.
- **Plugging in a display disturbs more than any window.** Windows plays the connect sound and every screen freezes for about a second, with no window in sight. It happened once per run during a game until the monitor stayed plugged for as long as the mode is on. Never add anything that plugs or changes a display during a run.
- **Windows' choice of screens switches the monitor off.** With "PC screen only" (Win+P), plugging the monitor in made Windows first apply what it last chose for that set of screens, lighting up a screen the user had turned off, then switch everything but the main screen off again within 2 to 3 s, the monitor too. The keeper gave up, the next run plugged in anew, and the user got the connect sound and their second screen flashing every half hour or so. A screen going to sleep does the same. Now the keeper reads the user's whole layout before it plugs in, and has Windows show exactly that plus the monitor at a corner, saved for that set (`SetDisplayConfig` with `SDC_SAVE_TO_DATABASE`). A layout where the monitor would be the main screen, share an edge with a screen of theirs, or show a copy of one is never saved. When Windows switches the monitor off it shows it again on its own path, with no new plug-in: back in about 150 ms. When a change of the user's screens puts it beside one of them, it goes back to a corner. Either at most three times a minute.
- **Windows can make the monitor the main screen.** Twice, as the user turned a screen on, Windows put the virtual monitor at 0,0, so the taskbar and new windows went where nobody sees them; the keeper only refused to keep that layout. Now it gives the main screen back: the one the user last had, if Windows shows it, else their largest, with their screens where they were around it, and notes that it did. It is never tried live on purpose: a change of main screen moves the user's taskbar and can move their windows for good. `a_monitor_made_the_main_screen_gives_it_back` covers it. Only the input desktop may change the layout, so this runs in the keeper, never from a hidden shell (`ERROR_ACCESS_DENIED`). `verify-monitor.ps1` switches it off as Windows would and fails if it does not come back.
- **A moment's doubt is not a decision.** A mode file nothing could read for 3 s read as off, and the keeper unplugged the monitor with the mode on (reproduced by holding the file). A virus scanner can hold a file like that. Only a file that says off, or none, may unplug it. `quietdesk status` says when the monitor is missing with the mode on, and `quietdesk on` plugs it back.
- **A full-screen game takes its screen away from DWM.** With screens of different rates, DWM's clock (`DwmGetCompositionTimingInfo`) then follows the other one: a game on a 165 Hz main screen made it read 60 Hz, and 144 Hz with the virtual monitor plugged in, even with another window in the foreground, so the foreground window does not tell. A check of display timing is wrong while that lasts, in or out of this mode, and must say so instead of reporting a regression. A window a check opens would also show over the game, and one taking the focus throws the user out of it.
- **Only a window's own thread can move it.** The keeper had a window that placed itself on the real screen off it in about 1 ms, as long as its program was free; a program busy for 1.5 s as its window opened kept it on the real screen for those 1.5 s, in every run, because the move waits in that program's queue. Faster looking changes nothing. DWM cloaking from another process is refused (`E_ACCESSDENIED`), and making the window transparent from outside works but makes it a layered window, which changes how DirectX presents: that would change the test. `verify-monitor.ps1` measures both cases.
- **A guard that looks on a timer is heard.** Looking every 0.2 s, a hidden test's tone played for 25 to 190 ms before its mute. Now Windows announces each new sound session (`IAudioSessionNotification`), which wakes the guard's own thread: it still writes a program down before muting it. Measure with a fresh copy of the program each time, as `verify-sound.ps1` does: Windows remembers a mute per program file, and a remembered one looks like the guard's.
- **The Claude app sees its own copy of the console setting.** The Claude desktop app is a packaged app (`C:\Program Files\WindowsApps\Claude_…`), and Windows gives the processes it starts (Claude Code, so the Bash tool, hooks and MCP servers, and everything they start) a private copy of `HKCU\Console\%%Startup`: writes from there go only into that copy, and the new consoles they open follow it. In that copy the classic-console values had been deleted at some point, so with the mode on, a hidden test's `CREATE_NEW_CONSOLE` from the Bash tool handed over to Windows Terminal on the owner's screen, during their game, while the real setting said classic console. The PowerShell tool runs the Store's PowerShell 7, another package, which sees the real setting, so every check run through it passed: `verify.ps1` too. The switch set only the copy it ran in. Now the first shim of every command puts its own copy right for the mode, with the backup of the user's choice in the same copy. To see a copy, read the key from the shell in question, never from another: `quietdesk status` there says what it holds. A process started by WMI (`Invoke-CimMethod Win32_Process Create`) sees the real one. Breakaway (`PROC_THREAD_ATTRIBUTE_DESKTOP_APP_POLICY`) does not leave the Claude app's copy. `a_copy_of_the_console_setting_the_switch_never_saw_follows_the_mode_from_the_next_command` covers it; the live proof is a hidden `CREATE_NEW_CONSOLE` from the Bash tool with a watcher on the real desktop (`quietdesk visible`).
- **A program the user wants to see can still open where nobody looks.** Blender started through `quietdesk visible` opened maximized on the virtual monitor, because Windows counts it as a display. To the user it looked as if nothing had opened. Now the keeper moves such windows to the main screen. It did so first only five times in a window's life, and judged once whether the window was the user's. Then Blender, which a session had opened while the monitor was there, was put back on the monitor later, maximized: the monitor goes away and comes back when a keeper restarts (each install) and when `verify-monitor.ps1` switches it off, and Windows moves windows off a screen that goes and back onto it when it returns. Nobody could bring it up from the taskbar. Now a window is moved whenever it lands there, at most twelve times a minute so a program that keeps putting itself back is not fought without end, and the list is read on every look. `verify-monitor.ps1` has a window that puts itself back seven times, and one whose program is put on the list while it is there; both failed against the old keeper. The shims decide by the command's text, the only thing they have before anything starts: `names_a_program` matches a word or the end of a path, never part of a word, so `blender-mcp` is not `blender`. `a_command_that_names_a_listed_program_runs_on_the_real_screen` and `the_powershell_tool_s_command_that_names_a_listed_program_runs_on_the_real_screen` cover both shims (the PowerShell tool's script comes in `CLAUDE_CODE_SHELL_LAUNCHER_SCRIPT`), and `verify-monitor.ps1` covers the moves with copies of a small window under listed and unlisted names.
- **Some dialogs ignore the desktop.** Windows raises a missing DLL's error box from `csrss` on the real screen, and the program waits for its answer. Only an error mode set before the program starts stops it. Set it on any new way of starting a program.
- **Windows remembers a mute per program file, as it stood when the session closed, and ignores an unmute after the process has ended.** So a muted test of a game makes the user's next launch of it start silent unless the guard fixes it. `muted-programs` in the state folder lists such programs, and only a session actually found muted clears an entry. So a tested game staying on that list is normal. Programs that no longer exist leave it when the tray starts.
- **A window is not a sign of a test.** A sound guard that looked for windows on the hidden desktop let a program with no window be heard (38 of 38 samples). A process's desktop is in `RTL_USER_PROCESS_PARAMETERS.DesktopInfo`, inherited by its children, and no Windows function returns it, so `desktop_of` reads it at fixed offsets. If they ever move, `verify-sound.ps1` fails.
- **A job does not hold a family.** The Store's PowerShell 7 lets its children out of any job, which left a game's window on the user's screen for 18 seconds while they played. The keeper follows parent links too, and checks start times so a reused id can never take hold of the user's window. Test this with a tiny window before any game.
- **A job can sit in one hierarchy only.** Once a process from outside any job has joined a job, Windows refuses it a process from inside another job (Access denied), even after it has emptied. Joined first from inside, it takes both. In the Claude app the Bash tool's processes sit in a job nothing may leave (`CREATE_BREAKAWAY_FROM_JOB` is refused too), and the children of the Store's PowerShell 7 sit in none. So with one job shared by all, everything tested through PowerShell passed, while `quietdesk monitor` from the Bash tool left a window that places itself on the user's screen, with no focus guard and no mute. Only the log said so. Now every command gets a new job, which takes a process from anywhere. Test through both, as `verify-monitor.ps1` does.
- **A killed keeper's monitor stays if anything pings the driver.** The driver's timer is shared: a new keeper pinging within a second kept the old monitor alive, beside the main screen, where the mouse can wander onto it. So a new keeper waits for any virtual monitor to go before it adds its own (one went in 1.2 s, two in 4.2 s). `quietdesk status` and `install.ps1` flag more than one. To count displays yourself use `GetSystemMetrics(SM_CMONITORS)`: .NET's `Screen.AllScreens` kept listing four after one had gone.
- **The keeper is `quietdesk.exe`, whoever starts it.** In the tray `current_exe` is `quietdesk-tray.exe`, whose second copy exits at its mutex with no word. So turning the mode on from the tray, or starting the tray with the mode on, never plugged the monitor in until `start_keeper` named its program. `install.ps1` now fails if the monitor does not come.
- **A long-lived helper must not inherit.** The keeper can run for days: inherited output handles would hold a command's pipeline open, and an inherited working folder could not be removed, such as a session's worktree. It starts in its own folder with no handles.
- **A window left behind holds a pipeline open:** a child of a hidden test inherits the output handle, and the shell command waits for ever. Close test windows when a test fails.
- **A window of the user's moved onto the virtual monitor counts as a test's,** and its program is muted. Nothing on the monitor is expected to be theirs.
- **There is one monitor, whatever the state folder.** `QUIETDESK_DIR` gives tests their own mode file, but the keeper's mutex covers the whole Windows session. A test that looked for the monitor's place in its own folder would not find one that is plugged in, and would run on the real screen.
- **A test run from a hidden shell inherits hidden.** Anything proving the real desktop must first move there, as `verify.ps1` does, or it fails for the wrong reason, and a check that a command stayed where it started passes for nothing. The shim tests start every command from the real desktop through the probe's launcher (`QUIETDESK_PROBE_LAUNCH`).

Not supported yet: Parsec's own app hosting a display on the same PC (QuietDesk takes any Parsec display for its own); Claude Code over SSH or as a service (no interactive desktop, so no hidden one); `.cmd` programs such as `npm` through `quietdesk hidden|visible|monitor`, which start a program and so need `cmd /c`; `quietdesk click` and `keys` reach the window itself, not a control in it (use `controls`, `press`, `settext`). PowerShell 7 installed for all users is found before the shim; `quietdesk status` says so. Windows PowerShell 5.1: with no PowerShell 7, install leaves the shim out, and the PowerShell tool is not hidden.
