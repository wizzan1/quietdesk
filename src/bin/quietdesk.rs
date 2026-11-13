//! `quietdesk`: turns QuietDesk on and off, runs one command on the screen you pick, and lets a session see, read and
//! operate windows without a mouse, a keyboard or the focus, on either desktop.

use std::path::Path;

use quietdesk::{Desktop, Mode, Place, Window, process_name, read_mode, set_mode, windows_on};
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_CLICK, EnumChildWindows, FindWindowW, GetClassNameW, IsWindow, IsWindowVisible, PostMessageW, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, WM_CHAR, WM_CLOSE, WM_GETTEXT, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_SETTEXT,
};

/// Prints a line. A reader that stopped listening, as in `quietdesk status | head`, is no failure: `println!` would panic here.
macro_rules! say {
    ($($text:tt)*) => {{
        use std::io::Write;
        let _ = writeln!(std::io::stdout(), $($text)*);
    }};
}

const USAGE: &str = "\
quietdesk on | off | toggle | status
    QuietDesk. While on, what Claude Code's commands open runs on a hidden desktop.
quietdesk visible <command...>
    Runs one command on the real screen, whatever the mode: for a test that needs it.
quietdesk hidden <command...>
    Runs one command hidden, whatever the mode.
quietdesk monitor <command...>
    Runs one command on a virtual monitor: out of sight, but to Windows on a real display. For what cannot run hidden.
quietdesk mute on | off
    Whether what runs out of sight is kept silent. Your own game and apps are never touched. On unless turned off.
quietdesk desktop
    Prints the desktop this shell runs on.
quietdesk start-tray | quit-tray
    Starts the tray icon on your screen, or asks it to close. Install and uninstall do this.
quietdesk exclude add | remove <program>
quietdesk exclude list
    Your own list of programs that stay on your screen even while the mode is on, such as an editor you work in.
";

const EXCLUDE_USAGE: &str = "\
quietdesk exclude add <program>       keeps a program on your screen even while the mode is on
quietdesk exclude remove <program>    hides it again, like everything else
quietdesk exclude list                shows your list

<program> is its name, with or without .exe, or its full path: blender, Blender.exe and
\"C:\\Program Files\\Blender Foundation\\Blender 5.2\\blender.exe\" all mean blender.exe.
A Claude Code command that starts a program on your list runs on your screen, from the next command on.";

const WINDOW_USAGE: &str = "\
Windows, wherever they run. <window> is 0xHANDLE, a process id, or part of the title, class or
program name of a window out of sight (a name never matches your own windows on the real screen):
quietdesk windows                       the windows out of sight
quietdesk shot <window> <file.png>      saves its picture; exit 3 if it comes out black
quietdesk controls <window>             its child controls: handle, class, text
quietdesk settext <0xCONTROL> <text>    sets a control's text
quietdesk press <0xCONTROL>             presses a button
quietdesk keys <window> <text>          sends characters
quietdesk click <window> <x> <y>        clicks at client coordinates
quietdesk close <window>                asks it to close";

/// Exit code of a capture that came out black: the file exists, but it does not show the window.
const BLACK_PICTURE: i32 = 3;
const USAGE_ERROR: i32 = 64;
const NOT_FOUND: i32 = 127;
const SEND_TIMEOUT_MS: u32 = 1000;
const TEXT_BUFFER: usize = 1024;

fn main() {
    let arguments: Vec<String> = std::env::args_os().skip(1).map(|word| word.to_string_lossy().into_owned()).collect();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let code = match words.as_slice() {
        ["on"] => switch(Mode::On),
        ["off"] => switch(Mode::Off),
        ["toggle"] => switch(if read_mode() == Mode::On { Mode::Off } else { Mode::On }),
        ["status"] => status(),
        ["visible", _, ..] => run_one_command(Place::Real),
        ["hidden", _, ..] => run_one_command(Place::Hidden),
        ["monitor", _, ..] => run_on_monitor(),
        [quietdesk::monitor::KEEPER_WORD] => quietdesk::monitor::keep(),
        ["mute", "on"] => set_test_sounds(true),
        ["mute", "off"] => set_test_sounds(false),
        ["desktop"] => report(quietdesk::current_desktop_name().map(|name| say!("{name}")).map_err(|e| e.to_string())),
        ["windows"] => report(list_background_windows()),
        ["shot", window, file] => shot(window, file),
        ["controls", window] => report(controls(window)),
        ["settext", control, text] => report(set_text(control, text)),
        ["press", control] => report(press(control)),
        ["keys", window, text] => report(keys(window, text)),
        ["click", window, x, y] => report(click(window, x, y)),
        ["close", window] => report(close(window)),
        ["quit-tray"] => report(quit_tray()),
        ["start-tray"] => report(start_tray()),
        ["exclude", "add", program] => report(exclude(program, true)),
        ["exclude", "remove", program] => report(exclude(program, false)),
        ["exclude", "list"] => {
            say!("{}", excluded_line());
            0
        }
        ["exclude", ..] => {
            eprintln!("{EXCLUDE_USAGE}");
            USAGE_ERROR
        }
        _ => {
            eprintln!("{USAGE}\n{WINDOW_USAGE}");
            USAGE_ERROR
        }
    };
    std::process::exit(code);
}

fn report(result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("quietdesk: {message}");
            1
        }
    }
}

fn mode_line(mode: Mode) -> &'static str {
    match mode {
        Mode::On => "QuietDesk is ON: what Claude Code's commands open now stays off your screen.",
        Mode::Off => "QuietDesk is OFF: windows open normally.",
    }
}

fn switch(mode: Mode) -> i32 {
    let result = set_mode(mode);
    say!("{}", mode_line(read_mode()));
    report(result.map_err(|e| format!("the switch reported a problem: {e}")))
}

fn sound_line() -> &'static str {
    match (quietdesk::sound::muting_enabled(), quietdesk::tray_running()) {
        (true, true) => "Test sounds: muted. What runs out of sight stays silent; your own game and apps keep their sound.",
        (true, false) => "Test sounds: NOT muted, because the tray app is not running. Start quietdesk-tray to mute them.",
        (false, _) => "Test sounds: allowed. What runs out of sight can be heard.",
    }
}

fn set_test_sounds(enabled: bool) -> i32 {
    let result = quietdesk::sound::set_muting(enabled);
    say!("{}", sound_line());
    report(result.map_err(|e| format!("could not save that: {e}")))
}

/// For install and uninstall: asks the tray to close, as its own window would be, so its sound guard gives back every mute it holds;
/// a kill would leave them for Windows to remember. Windows finds a window only on the desktop it is asked from, so the asking is
/// done from the real one, whichever this shell runs on.
fn quit_tray() -> Result<(), String> {
    let asked = quietdesk::on_desktop(false, || {
        let class = quietdesk::wide(quietdesk::TRAY_WINDOW_CLASS);
        let window = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
        if window.is_null() {
            return Ok(false);
        }
        if unsafe { PostMessageW(window, WM_CLOSE, 0, 0) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(true)
    });
    match asked {
        Ok(true) => Ok(()),
        Ok(false) => {
            say!("No tray is running.");
            Ok(())
        }
        Err(error) => Err(format!("could not ask the tray to close: {error}")),
    }
}

fn exclude(program: &str, excluded: bool) -> Result<(), String> {
    let (name, changed) = quietdesk::set_excluded(program, excluded).map_err(|error| format!("could not change the list: {error}"))?;
    say!(
        "{}",
        match (excluded, changed) {
            (true, true) => format!("{name} now opens on your screen even while the mode is on."),
            (true, false) => format!("{name} was already on the list."),
            (false, true) => format!("{name} runs hidden again while the mode is on."),
            (false, false) => format!("{name} was not on the list."),
        }
    );
    say!("{}", excluded_line());
    Ok(())
}

fn excluded_line() -> String {
    match quietdesk::excluded_programs().as_slice() {
        [] => "On your screen even with the mode on: nothing yet (quietdesk exclude add <program>)".to_string(),
        programs => format!("On your screen even with the mode on: {}", programs.join(", ")),
    }
}

/// Starts the tray on the real desktop as sign-in starts it, wherever this runs: apart from it, with nothing of the command that asked
/// for it, such as a test's own state folder.
fn start_tray() -> Result<(), String> {
    let tray = std::env::current_exe().map_err(|error| error.to_string())?.with_file_name("quietdesk-tray.exe");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().to_ascii_uppercase().starts_with("QUIETDESK_") {
            // Single-threaded here: nothing else runs yet.
            unsafe { std::env::remove_var(&name) };
        }
    }
    quietdesk::start_apart(&tray, "").map_err(|error| format!("could not start {}: {error}", tray.display()))
}

fn describe(window: &Window) -> String {
    let place = if window.is_hidden { "hidden" } else { "monitor" };
    format!("0x{:X}  {place}  pid {}  {}  {}x{}  \"{}\"", window.hwnd, window.pid, process_name(window.pid), window.width, window.height, window.title)
}

fn status() -> i32 {
    say!("{}", mode_line(read_mode()));
    say!("{}", sound_line());
    say!("This shell runs on desktop: {}", quietdesk::current_desktop_name().unwrap_or_else(|e| e.to_string()));
    say!("{}", excluded_line());
    // Per shell: a packaged app's commands, such as the Claude app's, see a copy of the console setting of their own.
    match (quietdesk::new_consoles_stay_with_their_command(), read_mode()) {
        (Ok(true), _) => say!("New consoles from this shell: the classic console, which stays with a hidden command"),
        (Ok(false), Mode::On) => say!(
            "New consoles from this shell: Windows Terminal, ON YOUR SCREEN even from a hidden command. The next command through QuietDesk sets it; `quietdesk on` does it now."
        ),
        (Ok(false), Mode::Off) => say!("New consoles from this shell: your own choice"),
        (Err(error), _) => say!("New consoles from this shell: the setting could not be read: {error}"),
    }
    if std::env::var_os(quietdesk::CLAUDE_VAR).is_some() {
        let own_dir = std::env::current_exe().ok().and_then(|exe| exe.parent().map(Path::to_path_buf));
        let powershell = own_dir.is_some_and(|dir| quietdesk::pwsh_shim_connected(&dir));
        let bash = std::env::var_os("CLAUDE_CODE_SHELL_PREFIX").is_some_and(|prefix| prefix.to_string_lossy().contains("quietdesk-bash"));
        say!("This session is connected: PowerShell {}, Bash {}", yes_no(powershell), yes_no(bash));
        if let Some(machine_wide) = quietdesk::machine_wide_pwsh().filter(|_| !powershell) {
            say!(
                "PowerShell 7 in {} is on the machine-wide PATH, which Windows searches before yours, so the PowerShell tool finds it before QuietDesk and its commands are not hidden. Install PowerShell 7 for your user only, or put QuietDesk's folder first on the machine-wide PATH.",
                machine_wide.display()
            );
        } else if !(powershell && bash) {
            say!("A session started before the install is not connected: restart the Claude app, then the session.");
        }
    }
    let virtual_monitors = quietdesk::monitor::virtual_display_count();
    if virtual_monitors > 1 {
        say!("Virtual monitors: {virtual_monitors}, MORE THAN ONE. The extra one can catch your mouse. `quietdesk off`, wait for them to go, then `quietdesk on`.");
    }
    match quietdesk::monitor::current_bounds() {
        Some(bounds) => say!("Virtual monitor: in use, at {},{} size {}x{}", bounds.left, bounds.top, bounds.right - bounds.left, bounds.bottom - bounds.top),
        None if quietdesk::monitor::driver_installed() && read_mode() == Mode::On => {
            say!("Virtual monitor: NOT plugged in, though the mode is on. The next run that needs it plugs it in, with the connect sound and a freeze. `quietdesk on` does it now.")
        }
        None if quietdesk::monitor::driver_installed() => say!("Virtual monitor: not in use"),
        None => say!("Virtual monitor: driver not installed, so `quietdesk monitor` runs on the real screen"),
    }
    match list_background_windows() {
        Ok(()) => {}
        Err(message) => say!("Windows out of sight could not be listed: {message}"),
    }
    if let Ok(problems) = std::fs::read_to_string(quietdesk::state_dir().join("problems.log"))
        && let Some(last) = problems.lines().last()
    {
        say!("Last problem noted: {last}");
    }
    0
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// `quietdesk visible <command...>` and `quietdesk hidden <command...>`: the rest of the line as written, so quoting survives.
fn run_one_command(place: Place) -> i32 {
    quietdesk::mark_decided();
    finish_command(quietdesk::run_and_wait(None, &command_after_the_word(), place))
}

/// What follows `quietdesk <word>`, as written.
fn command_after_the_word() -> Vec<u16> {
    let line = quietdesk::raw_command_line();
    let after_word = quietdesk::after_first_token(quietdesk::after_first_token(&line));
    let start = after_word.iter().position(|&c| c != b' ' as u16 && c != b'\t' as u16).unwrap_or(after_word.len());
    after_word[start..].to_vec()
}

fn finish_command(result: std::io::Result<u32>) -> i32 {
    match result {
        Ok(code) => code as i32,
        Err(error) => {
            eprintln!("quietdesk: cannot start that command: {error}");
            NOT_FOUND
        }
    }
}

/// `quietdesk monitor <command...>`. With no driver for a monitor, or with the mode off, the test runs on the real screen and says
/// so. SDL programs show without taking the focus, unless the caller has already decided that for itself.
fn run_on_monitor() -> i32 {
    const NO_FOCUS_HINT: &str = "SDL_WINDOW_NO_ACTIVATION_WHEN_SHOWN";
    quietdesk::mark_decided();
    if std::env::var_os(NO_FOCUS_HINT).is_none() {
        // Single-threaded here: this runs before anything is started.
        unsafe { std::env::set_var(NO_FOCUS_HINT, "1") };
    }
    let command = command_after_the_word();
    match quietdesk::monitor::Monitor::join() {
        Ok(monitor) => finish_command(monitor.run_and_wait(&command)),
        // A monitor that should be there and failed: on the real screen this would land in front of the user while the mode is on,
        // so it does not run, and the caller sees why. `quietdesk visible` runs it on the real screen on purpose.
        Err(error) if read_mode() == Mode::On && quietdesk::monitor::driver_installed() => {
            let message = format!("the virtual monitor failed ({error}), so this did not run; see quietdesk status");
            eprintln!("quietdesk: {message}");
            quietdesk::note_problem(&message);
            NOT_FOUND
        }
        Err(error) => {
            let message = format!("no virtual monitor ({error}), so this runs on the real screen");
            eprintln!("quietdesk: {message}");
            quietdesk::note_problem(&message);
            finish_command(quietdesk::run_and_wait(None, &command, Place::Real))
        }
    }
}

fn list_background_windows() -> Result<(), String> {
    let windows = quietdesk::background_windows().map_err(|e| e.to_string())?;
    say!("Windows out of sight: {}", windows.len());
    for window in &windows {
        say!("  {}", describe(window));
    }
    Ok(())
}

fn find_window(spec: &str) -> Result<Window, String> {
    let hidden = windows_on(&Desktop::hidden().map_err(|e| e.to_string())?);
    let real = windows_on(&Desktop::real().map_err(|e| e.to_string())?);
    if let Some(hex) = spec.strip_prefix("0x") {
        let handle = isize::from_str_radix(hex, 16).map_err(|_| format!("'{spec}' is not a window handle"))?;
        return hidden.into_iter().chain(real).find(|w| w.hwnd == handle).ok_or_else(|| format!("no window {spec}"));
    }
    if let Ok(pid) = spec.parse::<u32>() {
        // A process can own several windows: the largest is the one people mean.
        return hidden
            .into_iter()
            .chain(real)
            .filter(|w| w.pid == pid)
            .max_by_key(|w| i64::from(w.width) * i64::from(w.height))
            .ok_or_else(|| format!("process {pid} has no window"));
    }
    let needle = spec.to_lowercase();
    let matches = |w: &&Window| {
        w.title.to_lowercase().contains(&needle) || w.class.to_lowercase().contains(&needle) || process_name(w.pid).to_lowercase().contains(&needle)
    };
    let background = quietdesk::background_windows().map_err(|e| e.to_string())?;
    let found: Vec<&Window> = background.iter().filter(matches).collect();
    match found.as_slice() {
        [window] => Ok((*window).clone()),
        [] => Err(format!("no window out of sight matches '{spec}'; use a process id or 0xHANDLE for one on the real screen")),
        many => Err(format!("'{spec}' matches {} windows out of sight: {}", many.len(), many.iter().map(|w| format!("0x{:X}", w.hwnd)).collect::<Vec<_>>().join(", "))),
    }
}

fn shot(spec: &str, file: &str) -> i32 {
    let picture = find_window(spec).and_then(|window| quietdesk::capture(&window).map_err(|e| e.to_string()));
    let picture = match picture {
        Ok(picture) => picture,
        Err(message) => return report(Err(message)),
    };
    if let Err(error) = picture.save_png(Path::new(file)) {
        return report(Err(format!("cannot save {file}: {error}")));
    }
    if picture.is_black() {
        eprintln!(
            "quietdesk: the picture is black. An OpenGL window on the hidden desktop cannot be captured from outside: \
             use the app's own screenshot, or rerun it with `quietdesk monitor`."
        );
        return BLACK_PICTURE;
    }
    say!("{file}  {}x{}", picture.width, picture.height);
    0
}

fn control_handle(spec: &str) -> Result<isize, String> {
    let hex = spec.strip_prefix("0x").ok_or_else(|| format!("'{spec}' is not a 0xHANDLE; `quietdesk controls` lists them"))?;
    isize::from_str_radix(hex, 16).map_err(|_| format!("'{spec}' is not a 0xHANDLE"))
}

/// A bare control handle does not say which desktop it is on: tries the hidden one, then the real one.
fn on_the_controls_desktop(spec: &str, action: impl Fn(HandleOf) -> std::io::Result<()> + Sync) -> Result<(), String> {
    let handle = HandleOf(control_handle(spec)?);
    let attempt = |hidden| {
        quietdesk::on_desktop(hidden, || {
            if unsafe { IsWindow(handle.0 as HWND) } == 0 {
                return Err(std::io::Error::other("no such control"));
            }
            action(handle)
        })
    };
    attempt(true).or_else(|_| attempt(false)).map_err(|error| format!("control {spec}: {error}"))
}

/// A window handle on its way to the thread that has joined the window's desktop.
#[derive(Clone, Copy)]
struct HandleOf(isize);

/// A control's text. With a timeout: a hung program must not hang the session asking it.
fn text_of_control(hwnd: HWND) -> String {
    let mut buffer = [0u16; TEXT_BUFFER];
    let mut result = 0usize;
    unsafe {
        SendMessageTimeoutW(hwnd, WM_GETTEXT, buffer.len(), buffer.as_mut_ptr() as LPARAM, SMTO_ABORTIFHUNG, SEND_TIMEOUT_MS, &mut result);
    }
    quietdesk::text_of(&buffer)
}

fn controls(spec: &str) -> Result<(), String> {
    let window = find_window(spec)?;
    say!("{}", describe(&window));
    let parent = window.hwnd;
    let lines = quietdesk::on_desktop(window.is_hidden, || {
        let mut children: Vec<isize> = Vec::new();
        unsafe { EnumChildWindows(parent as HWND, Some(quietdesk::collect_handle), &mut children as *mut Vec<isize> as LPARAM) };
        Ok(children
            .into_iter()
            .map(|handle| {
                let hwnd = handle as HWND;
                let mut class = [0u16; TEXT_BUFFER];
                unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
                let shown = if unsafe { IsWindowVisible(hwnd) } == 0 { " (not shown)" } else { "" };
                format!("  0x{:X}  {}  \"{}\"{shown}", handle, quietdesk::text_of(&class), text_of_control(hwnd))
            })
            .collect::<Vec<String>>())
    })
    .map_err(|error| error.to_string())?;
    for line in lines {
        say!("{line}");
    }
    Ok(())
}

fn set_text(spec: &str, text: &str) -> Result<(), String> {
    let data = quietdesk::wide(text);
    on_the_controls_desktop(spec, |handle| {
        let mut result = 0usize;
        let sent = unsafe { SendMessageTimeoutW(handle.0 as HWND, WM_SETTEXT, 0, data.as_ptr() as LPARAM, SMTO_ABORTIFHUNG, SEND_TIMEOUT_MS, &mut result) };
        if sent == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
    })
}

fn post(window: &Window, message: u32, wparam: usize, lparam: LPARAM) -> Result<(), String> {
    quietdesk::post_to_window(window, message, wparam, lparam).map_err(|error| format!("could not reach the window: {error}"))
}

fn press(spec: &str) -> Result<(), String> {
    on_the_controls_desktop(spec, |handle| {
        if unsafe { PostMessageW(handle.0 as HWND, BM_CLICK, 0, 0) } == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
    })
}

fn keys(spec: &str, text: &str) -> Result<(), String> {
    let window = find_window(spec)?;
    for unit in text.encode_utf16() {
        post(&window, WM_CHAR, usize::from(unit), 0)?;
    }
    Ok(())
}

/// Client coordinates as Windows packs them into a mouse message: x in the low word, y in the high word.
fn mouse_position(x: u16, y: u16) -> LPARAM {
    ((isize::from(y as i16) as usize) << 16 | usize::from(x)) as LPARAM
}

fn click(spec: &str, x: &str, y: &str) -> Result<(), String> {
    let parse = |text: &str| text.parse::<u16>().map_err(|_| format!("'{text}' is not a coordinate in the window"));
    let position = mouse_position(parse(x)?, parse(y)?);
    let window = find_window(spec)?;
    const MK_LBUTTON: usize = 1;
    post(&window, WM_MOUSEMOVE, 0, position)?;
    post(&window, WM_LBUTTONDOWN, MK_LBUTTON, position)?;
    post(&window, WM_LBUTTONUP, 0, position)
}

fn close(spec: &str) -> Result<(), String> {
    post(&find_window(spec)?, WM_CLOSE, 0, 0)
}

#[cfg(test)]
mod tests {
    use super::mouse_position;

    #[test]
    fn a_mouse_position_keeps_x_and_y_in_their_words() {
        assert_eq!(mouse_position(10, 20) as usize & 0xFFFF_FFFF, 0x0014_000A);
        assert_eq!(mouse_position(40000, 1) as usize & 0xFFFF_FFFF, 0x0001_9C40);
    }
}
