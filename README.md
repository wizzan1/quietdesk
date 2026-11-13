# QuietDesk

Keeps what your AI coding agent's test runs open off your screen, on Windows.

When Claude Code tests its own work, the apps and games it starts pop up in front of you, take your keyboard focus and make
noise, run after run. With QuietDesk on, they run on a hidden Windows desktop instead: the same PC, files and network, and your
real GPU at full speed. The agent can still see, read and operate them, and you keep working, or playing.

It works with Claude Code today. The name is not tied to it: other agents, such as Codex, may follow.

**Status: early.** In daily use by its author since November 2026. Expect rough edges, and please report them.

## What you get

- **Every command the agent runs stays hidden** while the mode is on: the Bash and PowerShell tools, hooks and MCP servers
  (see the limits below). Nothing in your projects or in how the agent works changes.
- **A virtual monitor** for what cannot run hidden, such as a check of what the display shows, or an app that refuses a hidden
  desktop. Out of sight, but to Windows a real display.
- **Silent test runs:** what runs out of sight is muted the moment it opens its sound, before it is heard. Your own apps and games
  keep their sound.
- **Your focus back:** if a test window takes the focus anyway, it is given back.
- **One switch for every session:** the tray icon, `quietdesk on` and `quietdesk off`, or just ask the agent.
- **Your own exceptions:** programs you always want to see, such as an editor you work in beside the agent, stay on your screen.
- **No weaker tests:** the agent is told never to test less because of it, and how to handle what behaves differently hidden.

## Requirements

- Windows 10 or 11, x64. Home works.
- Claude Code, as the desktop app or the CLI, with Git for Windows.
- PowerShell 7, for the PowerShell tool. Without it Claude Code uses Windows PowerShell 5.1, which QuietDesk leaves alone: the
  install says so, and only that tool is not hidden. Installed for all users, PowerShell 7 sits on the machine-wide PATH, which
  Windows searches before yours: the PowerShell tool then finds it before QuietDesk, and the install and `quietdesk status` say so.
- [Rust](https://rustup.rs), to build it.
- Optional, for the virtual monitor: [Parsec's virtual display driver 0.41](https://builds.parsec.app/vdd/parsec-vdd-0.41.0.0.exe).
  Without it, `quietdesk monitor` runs its command on the real screen and says so. With it, a monitor that fails while the mode is
  on stops the command with the reason, rather than put it on your screen.

## Install

```
git clone https://github.com/wizzan1/quietdesk
cd quietdesk
pwsh -File tools\install.ps1
```

Then restart the Claude app, or your Claude Code terminals, once: running sessions read PATH when they start. The mode stays off until
you turn it on. Installing changes these, and `pwsh -File tools\uninstall.ps1` takes them back:

| Where | What |
| --- | --- |
| `%LOCALAPPDATA%\QuietDesk` | The programs, first on your user PATH, and the mode's state |
| `~\.claude\settings.json` | `env.CLAUDE_CODE_SHELL_PREFIX`, for the Bash tool. A copy of the file is kept from before the first install |
| `~\.claude\CLAUDE.md` | A short rule for the agent, between markers |
| `~\.claude\skills\quietdesk` | A skill: how to use it, and what differs when hidden |
| `HKCU\...\Run` | The tray icon, at sign-in |

## Turning it on and off

- Click the tray icon: a red disc with a white bar means on, a grey ring means off.
- Or tell any Claude Code session: "quiet desk on", "hide the windows", "show the windows".
- Or run `quietdesk on`, `quietdesk off`, `quietdesk toggle`, `quietdesk status`.

**Turn it on before you start a game.** Turning it on plugs in the virtual monitor, and turning it off unplugs it. Windows treats
that like a display cable: the connect sound, and in a full-screen game about a second's freeze. It happens once per flip; test
runs never plug anything while the mode is on, unless the monitor went away (Windows took it, and showing it again failed three
times in a minute), when the next run that needs it plugs it back in. Plugging it in leaves your own screens as they were.

A flip reaches every running session on its next command. Windows already open stay where they are: Windows cannot move a window
between desktops. To watch what runs hidden, right-click the tray icon and choose **Show what runs hidden**.

## Programs you always want to see

Some programs you want on your screen even while the mode is on, say a 3D editor you work in beside Claude. Put them on your
own list. Blender is only the example here: any program works, and the list starts empty.

| Command | What it does |
| --- | --- |
| `quietdesk exclude add blender` | Blender opens on your screen from now on, even with the mode on |
| `quietdesk exclude remove blender` | Blender is hidden again, like everything else |
| `quietdesk exclude list` | Shows your list. The tray menu and `quietdesk status` show it too |

- **Name the program as you like:** `blender`, `Blender.exe` or its full path all mean `blender.exe`.
- **What it changes:** when Claude Code runs a command that starts a program on your list, that command runs on your screen, as if
  the mode were off, so the program's windows open normally. Anything else in that same command shows too, so Claude is told to
  start such a program in a command of its own.
- **When it counts:** from Claude's next command. One already running stays where it started.
- **If Windows puts its window out of sight:** the virtual monitor counts as a screen to Windows, so a window can open there, or
  be put back there when the monitor returns. QuietDesk moves any window of a program on your list, or started through
  `quietdesk visible`, from the virtual monitor to your main screen, whenever it lands there, and restores one that opens
  minimized.

## How it works

Windows can run programs on a desktop that is never shown. A process started there stays there, with everything it starts. Two
small programs put Claude Code's commands on that desktop while the mode is on:

| Program | Where it sits | What it catches |
| --- | --- | --- |
| `pwsh.exe` | First on your user PATH: Claude Code finds PowerShell by name | The PowerShell tool |
| `quietdesk-bash.exe` | `CLAUDE_CODE_SHELL_PREFIX` | The Bash tool, hooks and MCP servers |

Both run below Claude Code's permission checks, so commands are not rewritten and those checks still see the real command. A Bash
script goes on to the same Git Bash that handed it over, exactly as written, with the environment Claude Code gave that Git Bash:
its output, errors, exit code, environment and shell options are what Git Bash gives with no QuietDesk, except that the shell is
one level deeper (`SHLVL`), which the tests check. An MCP server starts as Claude Code would start it without QuietDesk, as a
program with its arguments, a batch file such as `npx` through `cmd.exe`. With the mode off they only pass commands through. Your
own terminals are left alone: the PowerShell shim acts only for what Claude Code starts.

The virtual monitor belongs to a small keeper process. It has your main screen's size, sits at its corner so your mouse does not
wander onto it, and follows everything a command starts, so the command's windows end up there and the focus comes back to you.

## What behaves differently hidden

| What | Hidden | What to do |
| --- | --- | --- |
| Ordinary apps, Chrome | Work, and can be captured | Nothing |
| DirectX apps | Windows reports them as not visible; frames still reach a capture if the app keeps drawing | `quietdesk monitor` |
| OpenGL games | Run normally; a capture from outside is black | The game's own screenshots, or `quietdesk monitor` |
| Checks of what the display shows | Nothing reaches a display | `quietdesk monitor` |
| Checks that need the keyboard focus | Nothing hidden gets it | `quietdesk visible` |
| A program changing the resolution | Windows refuses it, so a hidden program can never change yours | `quietdesk visible` to test a real change |
| Edge | Exits at once, also headless | `quietdesk monitor`, or Chrome |

## What it cannot hide

- **A window on the virtual monitor whose program is busy as it opens.** A program that puts its own window on your screen has it
  moved off within milliseconds, but only once it is free: Windows lets only a window's own thread move it. One that is busy as its
  window opens, as a game is while it loads, keeps it on your screen until then. The one way to hide it from outside would change how
  it draws, and so what is tested.
- **Sound from the virtual monitor that starts before its program is known as a test's,** such as one started outside the command:
  muted at the guard's next look, within 0.2 seconds.
- **GPU load:** a hidden game still uses your GPU, so cap test frame rates if you play at the same time.
- **Questions:** a dialog waiting for an answer waits unseen, and the command looks hung. `quietdesk windows` and `quietdesk shot` show it.
- **A command on the virtual monitor that nothing can follow:** it does not start, and says so, rather than open on your screen.
- **Hooks written with `"args"`:** Claude Code starts them itself, not through `CLAUDE_CODE_SHELL_PREFIX`, so they are not hidden.
  Hooks written as a command line are.
- **An MCP server already running when you flip the mode** stays where it started: reconnect it (`/mcp`) to move it.
- **What another program opens for a command (not tested):** an elevation prompt, a program that hands over to a copy already on
  your screen, and what Windows opens on request, such as a web page, a folder or a Store app. Tools the Claude app starts itself,
  such as its preview browser, do not pass through QuietDesk.

## Long Bash commands

Git Bash, started by a Windows program, drops whatever follows about the first 8,190 characters of its command, and Claude Code
hands every Bash command over that way. So on Windows a Bash command longer than about 7,900 characters fails with `unexpected EOF
while looking for matching`, with or without QuietDesk. QuietDesk's prefix takes about 80 of those characters, and 4 more for each
single quote in the command. A long script can go in a file first, and be run from there.

## Not supported yet

- **Parsec's own app hosting a display on the same PC:** it uses the same driver, and QuietDesk takes any Parsec display for its own.
- **Claude Code over SSH or as a service:** with no interactive desktop there is no hidden one either.
- **`quietdesk hidden`, `visible` and `monitor` start a program, not a shell line:** `npm`, `npx` and other `.cmd` scripts need
  `cmd /c` in front, as they would from any Windows program.
- **`quietdesk click` and `keys` go to the window itself,** not to a control inside it: for a form or dialog use `controls`, then
  `press` or `settext`.

## Measured

On a Ryzen 9 7950X with an RTX 3060 Ti, a 2560x1440 165 Hz main screen and a second screen. With no game running, hidden matched
the real screen in every number:

| Question | Result |
| --- | --- |
| Frame rate of an OpenGL game (Reaction, on ioquake3) | 165 fps with vsync and 330 without, at 720p and 1440p, with the same frame times, hidden and on the real screen |
| A 33-step game test suite | All steps passed from a hidden shell; 16.8 minutes against 16.9 on the real screen |
| Timers | The same: `Sleep(1)` is a median of 1.50 ms in both places |
| A command's extra cost | About 30 ms through the shims; about 70 ms through `quietdesk monitor` against 57 ms with no monitor |
| A hidden test's sound | Muted as soon as it opens its sound, in 10 of 10 runs; looking every 0.2 s instead, it played for 25 to 190 ms |
| A window that opens on your screen, on the virtual monitor | Off it in about 1 ms (at most 19) when its program is free; as long as the program is busy otherwise |
| Background cost | The tray app and the keeper about 0.3 % of one core each |

What slows a test run is sharing the GPU with your own game, not the hiding: with a full-screen game using about 78 % of the GPU,
game test steps took about a third longer.

## Seeing and talking to hidden windows

```
quietdesk windows                       the windows out of sight
quietdesk shot <window> <file.png>      saves its picture; exit 3 if it comes out black
quietdesk controls <window>             its child controls: handle, class, text
quietdesk settext <0xCONTROL> <text>    sets a control's text
quietdesk press <0xCONTROL>             presses a button
quietdesk keys <window> <text>          sends characters
quietdesk click <window> <x> <y>        clicks at client coordinates
quietdesk close <window>                asks it to close
quietdesk visible <command...>          runs one command on the real screen
quietdesk hidden <command...>           runs one command hidden
quietdesk monitor <command...>          runs one command on the virtual monitor
quietdesk mute on | off                 whether what runs out of sight is kept silent
quietdesk exclude add | remove | list   programs that always open on your screen
```

`<window>` is `0xHANDLE`, a process id, or part of the title, class or program name of a window out of sight. A name never
matches your own windows on the real screen.

While the mode is on, new console windows, including your own, open in the classic console instead of Windows Terminal: a hidden
one would otherwise hand over to Windows Terminal and reach your screen. Turning it off restores your setting. The Claude app is a
packaged app, and Windows gives what it runs a copy of this setting of its own, so every command through QuietDesk also puts its
own copy right; `quietdesk status` says where a new console from that shell would open.

## Checks

```
cargo test
pwsh -File tools\verify-monitor.ps1
pwsh -File tools\verify-sound.ps1
pwsh -File tools\verify.ps1
```

- `cargo test` runs the programs against the paths they must keep working, and opens nothing on your screen.
- `verify-monitor.ps1`, with the mode on, proves `quietdesk monitor` from inside and outside a job, with windows nobody can see.
- `verify-sound.ps1` proves the sound guard with a real tone: it can be heard for a moment, opens small windows and restarts the tray.
- `verify.ps1` drives a real, throwaway Claude Code session through the installed mode with a cheap model, and reads the results
  from files the commands write, never from what the model says.

The skill (`skill/SKILL.md`) holds what was learned building this, including the traps. Read it before changing anything.

## License

MIT. See [LICENSE](LICENSE).
