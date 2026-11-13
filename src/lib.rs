//! QuietDesk: decides where Claude Code's commands run, and holds what the shims, the command line and the tray
//! app share. A Windows desktop object that is not the input desktop never reaches the screen, takes no focus and gets
//! no mouse or keyboard; a process started on it stays there with everything it starts.

pub mod monitor;
pub mod shell;
pub mod sound;

use std::ffi::{OsStr, OsString, c_void};
use std::io;
use std::iter::once;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};

use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_FILE_NOT_FOUND, GENERIC_ALL, HANDLE, HWND, LPARAM, RECT, SYSTEMTIME, TRUE, UNICODE_STRING, WAIT_FAILED,
};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject,
    GetDC, ReleaseDC, SelectObject,
};
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::Diagnostics::Debug::{GetErrorMode, ReadProcessMemory, SEM_FAILCRITICALERRORS, SetErrorMode};
use windows_sys::Win32::System::Environment::GetCommandLineW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_CREATED_NEW_KEY, REG_OPTION_NON_VOLATILE, REG_SZ,
    RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegDeleteKeyW, RegGetValueW, RegOpenKeyExW,
    RegSetKeyValueW,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::StationsAndDesktops::{
    CloseDesktop, CreateDesktopW, EnumDesktopWindows, GetThreadDesktop, GetUserObjectInformationW, HDESK, OpenDesktopW,
    SetThreadDesktop, UOI_NAME,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetCurrentProcess, GetCurrentThreadId, GetExitCodeProcess, INFINITE, OpenMutexW, OpenProcess, PROCESS_BASIC_INFORMATION,
    PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ, QueryFullProcessImageNameW, STARTUPINFOW,
    WaitForSingleObject,
};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GW_OWNER, GWL_EXSTYLE, GetClassNameW, GetClientRect, GetWindow, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WS_EX_TOOLWINDOW,
};

pub const HIDDEN_DESKTOP: &str = "QuietDesk";
pub const REAL_DESKTOP: &str = "Default";

/// Set for everything below the first shim: that one decided, and a whole test run stays where it started.
pub const DECIDED_VAR: &str = "QUIETDESK_DECIDED";
/// The shim folders a command already came through: see `shim_dirs_including`.
pub const SHIM_DIRS_VAR: &str = "QUIETDESK_SHIM_DIRS";
/// Claude Code sets this for what it starts; the user's own terminals do not have it.
pub const CLAUDE_VAR: &str = "CLAUDECODE";
/// Held by the tray app for as long as it runs; the sound guard runs in it.
pub const TRAY_MUTEX: &str = "QuietDeskTrayIcon";
/// The tray app's own window, on the real desktop: `quietdesk quit-tray` asks it to close.
pub const TRAY_WINDOW_CLASS: &str = "QuietDeskTrayWindow";

const CONSOLE_KEY: &str = r"Console\%%Startup";
const CONSOLE_VALUES: [&str; 2] = ["DelegationConsole", "DelegationTerminal"];
/// Windows Console Host. A new console from a hidden command otherwise hands over to Windows Terminal, which opens on
/// the user's screen.
const CONHOST_GUID: &str = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}";
/// The user's own choice, kept beside it in the same key. Windows gives a packaged app, such as the Claude app, a copy of this key
/// of its own: what that app starts, and the new consoles of what it starts, see the copy. So each copy needs the setting, and
/// keeps its own backup.
const CONSOLE_BEFORE: &str = "QuietDeskBefore";
/// Written last into the backup, so a backup cut short is never taken for the user's choice.
const CONSOLE_BEFORE_DONE: &str = "Saved";

/// A virus scanner can hold the mode file for a moment.
const MODE_READ_TRIES: u32 = 5;
const MODE_READ_RETRY: std::time::Duration = std::time::Duration::from_millis(5);

const PW_CLIENTONLY: u32 = 0x1;
const PW_RENDERFULLCONTENT: u32 = 0x2;
const TEXT_BUFFER: usize = 512;

pub fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref().encode_wide().chain(once(0)).collect()
}

pub fn text_of(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    OsString::from_wide(&buffer[..end]).to_string_lossy().into_owned()
}

/// `%LOCALAPPDATA%\QuietDesk`, or `QUIETDESK_DIR` (the tests).
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("QUIETDESK_DIR") {
        return PathBuf::from(dir);
    }
    // The library's own tests never touch the user's state: a problem they provoke on purpose is not one of the user's.
    if cfg!(test) {
        return std::env::temp_dir().join(format!("quietdesk-unit-tests-{}", std::process::id()));
    }
    installed_state_dir()
}

/// `%LOCALAPPDATA%\QuietDesk`, whatever the tests use: for what there is only one of, such as the virtual monitor. Not in an
/// agent's own folder: the mode is one switch for every agent on the PC.
fn installed_state_dir() -> PathBuf {
    let local = std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA is set for every Windows user");
    PathBuf::from(local).join("QuietDesk")
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    On,
    Off,
}

/// What a folder's mode file says. No file means off: nothing changes for a PC where the mode was never turned on. A file that
/// cannot be read right now says nothing, which is not the same as off.
fn mode_in(dir: &Path) -> io::Result<Mode> {
    match std::fs::read_to_string(dir.join("mode")) {
        Ok(text) => Ok(if text.trim() == "on" { Mode::On } else { Mode::Off }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Mode::Off),
        Err(error) => Err(error),
    }
}

/// Read on every command, so a flip reaches all running sessions at once. When the file stays unreadable the command still has to
/// run somewhere: off, where nothing is hidden by mistake.
pub fn read_mode() -> Mode {
    let dir = state_dir();
    let mut last_error = None;
    for _ in 0..MODE_READ_TRIES {
        match mode_in(&dir) {
            Ok(mode) => return mode,
            Err(error) => last_error = Some(error),
        }
        std::thread::sleep(MODE_READ_RETRY);
    }
    if let Some(error) = last_error {
        note_problem(&format!("could not read the mode file, so it counted as off for a moment: {error}"));
    }
    Mode::Off
}

/// For what must not act on a moment's doubt: an unreadable file says nothing, so this is false.
pub(crate) fn mode_is_certainly_off() -> bool {
    matches!(mode_in(&state_dir()), Ok(Mode::Off))
}

/// The switch itself: the mode file and, with it, where new consoles open. Turning on changes consoles first, so no
/// command runs hidden while its new consoles could still reach the screen. Turning off writes the mode first: a console
/// setting that cannot be restored must never leave the user's windows hidden.
pub fn set_mode(mode: Mode) -> io::Result<()> {
    let dir = state_dir();
    std::fs::create_dir_all(&dir)?;
    match mode {
        Mode::On => {
            keep_new_consoles_with(Mode::On)?;
            write_mode_file(&dir, "on\n")?;
            monitor::stay_plugged_in_while_the_mode_is_on();
            Ok(())
        }
        Mode::Off => {
            write_mode_file(&dir, "off\n")?;
            keep_new_consoles_with(Mode::Off)
        }
    }
}

/// A rename, so a shim reading at the same moment sees the old word or the new one, never half a file.
fn write_mode_file(dir: &Path, word: &str) -> io::Result<()> {
    replace_file(&dir.join("mode"), word)
}

/// Written aside, then put in place in one step: a crash or a reader in between never sees a half-written file.
fn replace_file(path: &Path, text: &str) -> io::Result<()> {
    let aside = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&aside, text)?;
    std::fs::rename(&aside, path)
}

/// `QUIETDESK_CONSOLE_KEY` lets the tests use a key of their own: they must never touch the user's console choice.
fn console_key() -> String {
    std::env::var("QUIETDESK_CONSOLE_KEY").unwrap_or_else(|_| CONSOLE_KEY.to_string())
}

fn read_console_value(name: &str) -> io::Result<Option<String>> {
    read_value_in(&console_key(), name)
}

fn write_console_value(name: &str, text: &str) -> io::Result<()> {
    write_value_in(&console_key(), name, text)
}

fn delete_console_value(name: &str) -> io::Result<()> {
    let (key, value) = (wide(console_key()), wide(name));
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) };
    match status {
        0 | ERROR_FILE_NOT_FOUND => Ok(()),
        other => Err(io::Error::from_raw_os_error(other as i32)),
    }
}

/// Where an earlier version kept the backup, in a file: one copy of the setting only, and not always the one that matters.
fn legacy_console_backup(dir: &Path) -> PathBuf {
    dir.join("console-before")
}

fn backup_key() -> String {
    format!(r"{}\{CONSOLE_BEFORE}", console_key())
}

fn read_value_in(key: &str, name: &str) -> io::Result<Option<String>> {
    let (key, value) = (wide(key), wide(name));
    let mut buffer = [0u16; TEXT_BUFFER];
    let mut bytes = (buffer.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), buffer.as_mut_ptr().cast(), &mut bytes)
    };
    match status {
        0 => Ok(Some(text_of(&buffer))),
        ERROR_FILE_NOT_FOUND => Ok(None),
        other => Err(io::Error::from_raw_os_error(other as i32)),
    }
}

fn write_value_in(key: &str, name: &str, text: &str) -> io::Result<()> {
    let (key, value, data) = (wide(key), wide(name), wide(text));
    let status = unsafe {
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32)
    };
    if status == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(status as i32)) }
}

fn backup_exists() -> bool {
    let key = wide(backup_key());
    let mut handle: HKEY = null_mut();
    let found = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_READ, &mut handle) } == 0;
    if found {
        unsafe { RegCloseKey(handle) };
    }
    found
}

/// Creates the backup key, or says it was there: one step, so of two commands at once only one takes the backup.
fn create_backup_key() -> io::Result<bool> {
    let key = wide(backup_key());
    let (mut handle, mut disposition): (HKEY, u32) = (null_mut(), 0);
    let status = unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, null(), &mut handle, &mut disposition)
    };
    if status != 0 {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    unsafe { RegCloseKey(handle) };
    Ok(disposition == REG_CREATED_NEW_KEY)
}

fn delete_backup_key() -> io::Result<()> {
    let key = wide(backup_key());
    match unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr()) } {
        0 | ERROR_FILE_NOT_FOUND => Ok(()),
        other => Err(io::Error::from_raw_os_error(other as i32)),
    }
}

/// New consoles in this process's copy of the setting open in the classic console, which stays with a hidden command. The user's
/// choice is backed up first, once per copy, so turning on twice cannot overwrite it with our own value.
fn send_new_consoles_to_conhost(dir: &Path) -> io::Result<()> {
    let now: Vec<Option<String>> = CONSOLE_VALUES.iter().map(|name| read_console_value(name)).collect::<io::Result<_>>()?;
    if now.iter().all(|value| value.as_deref() == Some(CONHOST_GUID)) {
        return Ok(());
    }
    // Read before the backup is claimed: of two commands at once, the one that claims it has read the user's own values.
    if create_backup_key()? {
        for (name, value) in CONSOLE_VALUES.iter().zip(&now) {
            if let Some(value) = value {
                write_value_in(&backup_key(), name, value)?;
            }
        }
        write_value_in(&backup_key(), CONSOLE_BEFORE_DONE, "1")?;
    }
    for name in CONSOLE_VALUES {
        write_console_value(name, CONHOST_GUID)?;
    }
    // The earlier version's backup is now this copy's, taken above.
    let _ = std::fs::remove_file(legacy_console_backup(dir));
    Ok(())
}

/// Gives this process's copy of the setting back what it held before the mode was on. A value that is no longer ours is a choice the
/// user made meanwhile: theirs stays.
fn restore_new_consoles(dir: &Path) -> io::Result<()> {
    let legacy = legacy_console_backup(dir);
    if !backup_exists() {
        // Only an earlier version's backup: restored as that version did.
        let saved = match std::fs::read_to_string(&legacy) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error),
        };
        let mut lines = saved.lines();
        for name in CONSOLE_VALUES {
            let before = lines.next().unwrap_or("");
            if read_console_value(name)?.as_deref() == Some(CONHOST_GUID) {
                match before {
                    "" => delete_console_value(name)?,
                    value => write_console_value(name, value)?,
                }
            }
        }
        return std::fs::remove_file(legacy);
    }
    // A backup cut short says nothing of the user's choice: then ours stays, the safe side.
    if read_value_in(&backup_key(), CONSOLE_BEFORE_DONE)?.is_some() {
        for name in CONSOLE_VALUES {
            if read_console_value(name)?.as_deref() != Some(CONHOST_GUID) {
                continue;
            }
            match read_value_in(&backup_key(), name)? {
                Some(value) => write_console_value(name, &value)?,
                None => delete_console_value(name)?,
            }
        }
    }
    delete_backup_key()
}

/// Puts this process's copy of the console setting where the mode wants it. The switch does it where it runs, and every command's
/// first shim where that command runs: a packaged app's commands, such as the Claude app's, see a copy of their own.
pub fn keep_new_consoles_with(mode: Mode) -> io::Result<()> {
    match mode {
        Mode::On => send_new_consoles_to_conhost(&state_dir()),
        Mode::Off if backup_exists() || legacy_console_backup(&state_dir()).exists() => restore_new_consoles(&state_dir()),
        Mode::Off => Ok(()),
    }
}

/// Where a new console from this process opens, by its copy of the setting: the classic console stays with a hidden command,
/// anything else (Windows Terminal, or Windows deciding, which is Windows Terminal on Windows 11) opens on the user's screen.
pub fn new_consoles_stay_with_their_command() -> io::Result<bool> {
    Ok(read_console_value("DelegationConsole")?.as_deref() == Some(CONHOST_GUID))
}

/// A desktop handle; the hidden desktop exists for as long as a handle or a process holds it.
pub struct Desktop {
    handle: HDESK,
    pub is_hidden: bool,
}

impl Desktop {
    /// Opens the hidden desktop, creating it the first time.
    pub fn hidden() -> io::Result<Self> {
        let name = wide(HIDDEN_DESKTOP);
        let handle = unsafe { CreateDesktopW(name.as_ptr(), null(), null(), 0, GENERIC_ALL, null()) };
        if handle.is_null() { Err(io::Error::last_os_error()) } else { Ok(Self { handle, is_hidden: true }) }
    }

    pub fn real() -> io::Result<Self> {
        let name = wide(REAL_DESKTOP);
        let handle = unsafe { OpenDesktopW(name.as_ptr(), 0, 0, GENERIC_ALL) };
        if handle.is_null() { Err(io::Error::last_os_error()) } else { Ok(Self { handle, is_hidden: false }) }
    }
}

impl Drop for Desktop {
    fn drop(&mut self) {
        unsafe { CloseDesktop(self.handle) };
    }
}

/// The name of the desktop this process runs on.
pub fn current_desktop_name() -> io::Result<String> {
    let mut buffer = [0u16; TEXT_BUFFER];
    let mut needed = 0u32;
    let ok = unsafe {
        GetUserObjectInformationW(
            GetThreadDesktop(GetCurrentThreadId()),
            UOI_NAME,
            buffer.as_mut_ptr().cast(),
            (buffer.len() * 2) as u32,
            &mut needed,
        )
    };
    if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(text_of(&buffer)) }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Place {
    /// Wherever this process runs.
    Inherit,
    Hidden,
    Real,
}

/// Programs the user wants on their screen even while the mode is on: things to look at, not tests, such as Blender. One program
/// file name a line, as `quietdesk exclude` and the tray keep them.
fn excluded_file() -> PathBuf {
    state_dir().join("always-on-your-screen")
}

/// A program's file name as the list keeps it: the last part of a path, in lower case, with ".exe" when it has no extension. None
/// for nothing at all.
pub fn program_name(given: &str) -> Option<String> {
    let name = given.trim().trim_matches(|c| c == '"' || c == '\'').rsplit(['/', '\\']).next()?.trim().to_lowercase();
    if name.is_empty() {
        return None;
    }
    Some(if Path::new(&name).extension().is_some() { name } else { format!("{name}.exe") })
}

/// The programs on the list, as kept.
pub fn excluded_programs() -> Vec<String> {
    std::fs::read_to_string(excluded_file()).unwrap_or_default().lines().filter_map(program_name).collect()
}

/// Puts a program on the list or takes it off; its name as kept, and whether the list changed.
pub fn set_excluded(given: &str, excluded: bool) -> io::Result<(String, bool)> {
    let name = program_name(given).ok_or_else(|| io::Error::other("no program named"))?;
    let mut programs = excluded_programs();
    let had = programs.contains(&name);
    if excluded == had {
        return Ok((name, false));
    }
    if excluded {
        programs.push(name.clone());
        programs.sort();
    } else {
        programs.retain(|program| program != &name);
    }
    std::fs::create_dir_all(state_dir())?;
    replace_file(&excluded_file(), &programs.iter().map(|program| format!("{program}\n")).collect::<String>())?;
    Ok((name, true))
}

/// Whether a command names one of these programs: a word of it, or the last part of a path in it, is that program's file name,
/// with or without ".exe". `blender.exe file.blend`, `& "C:\Program Files\Blender Foundation\Blender 5.2\blender.exe"` and
/// `blender --python x.py` name blender.exe; a folder called BlenderProject does not.
pub fn names_a_program(command: &str, programs: &[String]) -> bool {
    if programs.is_empty() {
        return false;
    }
    let bare = |name: &str| name.strip_suffix(".exe").unwrap_or(name).to_string();
    let wanted: Vec<String> = programs.iter().map(|program| bare(program)).collect();
    // Paths may hold spaces, so a word is cut at what ends a path only after its last separator.
    command
        .split(['"', '\'', '`', ';', '&', '|', '(', ')', '<', '>', '\n', '\r', '\t'])
        .flat_map(|piece| {
            let tail = piece.rsplit(['/', '\\']).next().unwrap_or(piece);
            tail.split_whitespace().take(1).chain(piece.split_whitespace()).map(str::to_string).collect::<Vec<_>>()
        })
        .map(|word| bare(&word.rsplit(['/', '\\']).next().unwrap_or(&word).to_lowercase()))
        .any(|word| wanted.contains(&word))
}

/// Whether a command names a program on the user's list.
pub fn names_an_excluded_program(command: &str) -> bool {
    names_a_program(command, &excluded_programs())
}

/// Programs never offered for the list: QuietDesk's own, and Windows' own, which host other programs' windows or the shell.
const NEVER_OFFERED: [&str; 9] = [
    "quietdesk.exe",
    "quietdesk-tray.exe",
    "quietdesk-bash.exe",
    "explorer.exe",
    "applicationframehost.exe",
    "shellexperiencehost.exe",
    "startmenuexperiencehost.exe",
    "searchhost.exe",
    "textinputhost.exe",
];

/// The programs to offer for the list, from those with a window open: each once, by its name as the list keeps it, in order, and
/// none already on the list or never offered.
pub fn programs_to_offer(open: &[String], listed: &[String]) -> Vec<String> {
    let mut offered: Vec<String> = open
        .iter()
        .filter_map(|name| program_name(name))
        .filter(|name| !listed.contains(name) && !NEVER_OFFERED.contains(&name.as_str()))
        .collect();
    offered.sort();
    offered.dedup();
    offered
}

/// A window as the taskbar would show it: no owner, not a tool window, with a title, and not cloaked (a Store app's window that is
/// not on screen).
fn is_app_window(window: &Window) -> bool {
    let hwnd = window.hwnd as HWND;
    let extended = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let mut cloaked: u32 = 0;
    let cloak_read = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED as u32, (&mut cloaked as *mut u32).cast(), size_of::<u32>() as u32) };
    !window.title.trim().is_empty()
        && unsafe { GetWindow(hwnd, GW_OWNER) }.is_null()
        && extended & WS_EX_TOOLWINDOW == 0
        && !(cloak_read == 0 && cloaked != 0)
}

/// The programs with a window open on a desktop, to offer for the list: on the real one, the user's own. A window on the virtual
/// monitor is a test's, and not offered.
pub fn open_programs_on(desktop: &Desktop) -> Vec<String> {
    let monitor = monitor::current_bounds();
    let names: Vec<String> = windows_on(desktop)
        .into_iter()
        .filter(|window| is_app_window(window) && !monitor.as_ref().is_some_and(|bounds| monitor::holds(bounds, window)))
        .map(|window| process_name(window.pid))
        .collect();
    programs_to_offer(&names, &excluded_programs())
}

pub fn decide_place(require_claude: bool) -> Place {
    decide_place_for(require_claude, "")
}

/// Where a command Claude Code starts belongs. Only the first shim of a run decides; `require_claude` keeps the user's own `pwsh`
/// out of it, because the pwsh shim is on the user's PATH. By the command's own text: a command that names a program on the user's
/// list runs on their screen even while the mode is on, as a whole.
pub fn decide_place_for(require_claude: bool, command: &str) -> Place {
    if std::env::var_os(DECIDED_VAR).is_some() {
        return Place::Inherit;
    }
    if require_claude && std::env::var_os(CLAUDE_VAR).is_none() {
        return Place::Inherit;
    }
    mark_decided();
    let mode = read_mode();
    if let Err(error) = keep_new_consoles_with(mode) {
        note_problem(&format!("could not set where new consoles open, for this command's copy of the setting: {error}"));
    }
    match mode {
        Mode::On if names_an_excluded_program(command) => Place::Real,
        Mode::On => Place::Hidden,
        Mode::Off => Place::Inherit,
    }
}

pub fn mark_decided() {
    // Single-threaded here: this runs before anything is started.
    unsafe { std::env::set_var(DECIDED_VAR, "1") };
}

/// A problem that must not stop the command: it runs anyway, and `quietdesk status` shows this.
pub fn note_problem(message: &str) {
    use std::io::Write;
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut now: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut now) };
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("problems.log")) {
        let _ = writeln!(file, "{}-{:02}-{:02} {:02}:{:02}:{:02} {message}", now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond);
    }
}

unsafe extern "system" fn leave_ctrl_events_to_the_child(_event: u32) -> i32 {
    // The child gets the same event and decides; its exit code is what this process returns.
    TRUE
}

/// Starts a command line with this process's console, standard handles and environment, on the given desktop, and
/// waits for it. Returns its exit code. `application` is the exact program, or None to let Windows search as usual.
pub fn run_and_wait(application: Option<&Path>, command_line: &[u16], place: Place) -> io::Result<u32> {
    run_and_wait_in(application, command_line, place, None)
}

/// The same, with this environment block instead of this process's own, when one is given.
pub fn run_and_wait_in(application: Option<&Path>, command_line: &[u16], place: Place, environment: Option<&[u16]>) -> io::Result<u32> {
    let hidden = match place {
        Place::Hidden => match Desktop::hidden() {
            Ok(desktop) => Some(desktop),
            Err(error) => {
                note_problem(&format!("could not open the hidden desktop, ran on the current one: {error}"));
                None
            }
        },
        _ => None,
    };
    let mut desktop_name = match (place, &hidden) {
        (Place::Hidden, Some(_)) => Some(wide(hidden_desktop_path())),
        (Place::Real, _) => Some(wide(format!(r"WinSta0\{REAL_DESKTOP}"))),
        _ => None,
    };
    let mut line: Vec<u16> = command_line.iter().copied().take_while(|&c| c != 0).chain(once(0)).collect();
    let application = application.map(|path| wide(path.as_os_str()));

    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    startup.lpDesktop = desktop_name.as_mut().map_or(null_mut(), |name| name.as_mut_ptr());
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };

    if place == Place::Hidden {
        no_system_error_dialogs();
    }
    leave_ctrl_events_to_the_child_while_waiting();
    let started = unsafe {
        CreateProcessW(
            application.as_ref().map_or(null(), |path| path.as_ptr()),
            line.as_mut_ptr(),
            null(),
            null(),
            TRUE,
            if environment.is_some() { windows_sys::Win32::System::Threading::CREATE_UNICODE_ENVIRONMENT } else { 0 },
            environment.map_or(null(), |block| block.as_ptr().cast()),
            null(),
            &startup,
            &mut process,
        )
    };
    if started == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe { CloseHandle(process.hThread) };
    wait_and_close(process.hProcess)
}

fn hidden_desktop_path() -> String {
    format!(r"WinSta0\{HIDDEN_DESKTOP}")
}

/// A program that cannot start, such as one with a DLL missing, makes Windows raise a dialog on the real screen, whatever
/// desktop the program is on, and the program waits for its answer. With this mode, which everything started below here
/// inherits, it exits with its error code instead (0xC0000135 for that DLL), which is what a test needs to see anyway.
fn no_system_error_dialogs() {
    unsafe { SetErrorMode(GetErrorMode() | SEM_FAILCRITICALERRORS) };
}

fn leave_ctrl_events_to_the_child_while_waiting() {
    unsafe { SetConsoleCtrlHandler(Some(leave_ctrl_events_to_the_child), TRUE) };
}

/// Waits for a process, closes its handle and returns its exit code.
fn wait_and_close(process: HANDLE) -> io::Result<u32> {
    unsafe {
        if WaitForSingleObject(process, INFINITE) == WAIT_FAILED {
            let error = io::Error::last_os_error();
            CloseHandle(process);
            return Err(error);
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(process, &mut code);
        let error = io::Error::last_os_error();
        CloseHandle(process);
        if ok == 0 { Err(error) } else { Ok(code) }
    }
}

/// This process's raw command line, UTF-16, without the terminating zero.
pub fn raw_command_line() -> Vec<u16> {
    let line = unsafe { GetCommandLineW() };
    let mut length = 0;
    while unsafe { *line.add(length) } != 0 {
        length += 1;
    }
    unsafe { std::slice::from_raw_parts(line, length) }.to_vec()
}

const SPACE: u16 = b' ' as u16;
const TAB: u16 = b'\t' as u16;
const QUOTE: u16 = b'"' as u16;

/// What follows the first token, exactly as written, spacing included. The token ends as Windows ends a program name:
/// at the closing quote when it opens with one, otherwise at the first space or tab.
pub fn after_first_token(line: &[u16]) -> &[u16] {
    let mut i = 0;
    while i < line.len() && (line[i] == SPACE || line[i] == TAB) {
        i += 1;
    }
    if i < line.len() && line[i] == QUOTE {
        i += 1;
        while i < line.len() && line[i] != QUOTE {
            i += 1;
        }
        i = (i + 1).min(line.len());
    } else {
        while i < line.len() && line[i] != SPACE && line[i] != TAB {
            i += 1;
        }
    }
    &line[i..]
}

/// Everything after this program's own name, untouched, so a shim forwards arguments byte for byte.
pub fn arguments_after_program() -> Vec<u16> {
    let line = raw_command_line();
    after_first_token(&line).to_vec()
}

/// `"program"<arguments>`, ready for `run_and_wait`.
pub fn command_line_for(program: &Path, arguments: &[u16]) -> Vec<u16> {
    let mut line: Vec<u16> = vec![QUOTE];
    line.extend(program.as_os_str().encode_wide());
    line.push(QUOTE);
    if !arguments.is_empty() && arguments[0] != SPACE && arguments[0] != TAB {
        line.push(SPACE);
    }
    line.extend_from_slice(arguments);
    line
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let normal = |path: &Path| path.to_string_lossy().trim_end_matches('\\').to_lowercase();
    normal(a) == normal(b)
}

/// Records this shim's folder for any shim started below it, and returns every shim folder so far. With two shims on
/// PATH, each would find the other as "the next pwsh" and they would start each other without end. A folder in this list
/// is never started again, so a chain of shims can only end at the real program or at an error.
pub fn shim_dirs_including(own_dir: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os(SHIM_DIRS_VAR).map(|list| std::env::split_paths(&list).collect()).unwrap_or_default();
    if !dirs.iter().any(|dir| same_dir(dir, own_dir)) {
        dirs.push(own_dir.to_path_buf());
    }
    if let Ok(list) = std::env::join_paths(&dirs) {
        // Single-threaded here: this runs before anything is started.
        unsafe { std::env::set_var(SHIM_DIRS_VAR, list) };
    }
    dirs
}

/// The first `name` on PATH outside the `skip` folders. Looks at the entry itself: the Store's pwsh is an app execution
/// alias, which a metadata call that follows links cannot read.
pub fn next_on_path(name: &str, skip: &[PathBuf]) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() || skip.iter().any(|other| same_dir(&dir, other)) {
            continue;
        }
        let candidate = dir.join(name);
        if std::fs::symlink_metadata(&candidate).is_ok() {
            return Some(candidate);
        }
    }
    None
}

/// Whether the command running this was connected through the pwsh shim. PATH alone cannot say: PowerShell puts its own
/// folder first for what it starts, so inside the PowerShell tool the shim looks absent. The shim's record says it came through.
pub fn pwsh_shim_connected(own_dir: &Path) -> bool {
    let recorded = std::env::var_os(SHIM_DIRS_VAR).is_some_and(|list| std::env::split_paths(&list).any(|dir| same_dir(&dir, own_dir)));
    recorded
        || std::env::var_os("PATH").is_some_and(|path| {
            std::env::split_paths(&path)
                .find(|dir| std::fs::symlink_metadata(dir.join("pwsh.exe")).is_ok())
                .is_some_and(|dir| same_dir(&dir, own_dir))
        })
}

/// A PowerShell 7 on the machine-wide PATH, which Windows searches before the user's own: installed for all users, it is found
/// before the shim, and the PowerShell tool's commands are not hidden.
pub fn machine_wide_pwsh() -> Option<PathBuf> {
    let (key, value) = (wide(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"), wide("Path"));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let mut bytes = 0u32;
    let sized = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), flags, null_mut(), null_mut(), &mut bytes) };
    if sized != 0 {
        return None;
    }
    let mut buffer = vec![0u16; bytes as usize / 2 + 1];
    let read = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), flags, null_mut(), buffer.as_mut_ptr().cast(), &mut bytes)
    };
    if read != 0 {
        return None;
    }
    std::env::split_paths(&text_of(&buffer)).find(|dir| dir.join("pwsh.exe").is_file())
}

/// Runs a shim's real program and returns the exit code for the process to end with. A shim that cannot start it says
/// so on stderr with 127, the code a shell gives a missing command.
pub fn forward(real: &Path, arguments: &[u16], place: Place) -> i32 {
    forward_in(real, arguments, place, None)
}

/// The same, with this environment block instead of this process's own, when one is given.
pub fn forward_in(real: &Path, arguments: &[u16], place: Place, environment: Option<&[u16]>) -> i32 {
    let line = command_line_for(real, arguments);
    exit_code_of(run_and_wait_in(Some(real), &line, place, environment), &real.display().to_string())
}

/// Starts a program on the real desktop and leaves it to run on its own, for as long as it likes: with none of this process's
/// handles, so it holds no command's output pipe open, and in QuietDesk's own folder, so it holds no working folder such as a
/// session's worktree.
pub fn start_apart(program: &Path, arguments: &str) -> io::Result<()> {
    let application = wide(program.as_os_str());
    let mut line = wide(format!("\"{}\" {arguments}", program.display()));
    let mut desktop = wide(format!(r"WinSta0\{REAL_DESKTOP}"));
    let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    startup.lpDesktop = desktop.as_mut_ptr();
    let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    std::fs::create_dir_all(installed_state_dir())?;
    let folder = wide(installed_state_dir().as_os_str());
    let flags = windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    let started = unsafe {
        CreateProcessW(application.as_ptr(), line.as_mut_ptr(), null(), null(), 0, flags, null(), folder.as_ptr(), &startup, &mut process)
    };
    if started == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}

/// Starts a whole command line with this program, and waits for it.
pub fn run_command_line(program: &Path, line: &str, place: Place) -> i32 {
    let wide: Vec<u16> = line.encode_utf16().collect();
    exit_code_of(run_and_wait(Some(program), &wide, place), line)
}

/// What a shell returns: the program's own exit code, or 127 when it could not be started.
fn exit_code_of(result: io::Result<u32>, what: &str) -> i32 {
    match result {
        Ok(code) => code as i32,
        Err(error) => {
            eprintln!("quietdesk: cannot start {what}: {error}");
            127
        }
    }
}

/// The id and program of the process that started this one. The shims are waited for, so it still runs and its id is still its own.
pub fn parent_process() -> Option<(u32, String)> {
    let mut basic: PROCESS_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let status = unsafe {
        NtQueryInformationProcess(
            GetCurrentProcess(),
            ProcessBasicInformation,
            (&mut basic as *mut PROCESS_BASIC_INFORMATION).cast(),
            size_of::<PROCESS_BASIC_INFORMATION>() as u32,
            null_mut(),
        )
    };
    if status != 0 {
        return None;
    }
    let pid = basic.InheritedFromUniqueProcessId as u32;
    let path = process_path(pid);
    (!path.is_empty()).then_some((pid, path))
}

/// A running process's whole command line, as Windows keeps it.
pub fn command_line_of(pid: u32) -> Option<String> {
    const PROCESS_COMMAND_LINE_INFORMATION: i32 = 60;
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut needed = 0u32;
        NtQueryInformationProcess(process, PROCESS_COMMAND_LINE_INFORMATION, null_mut(), 0, &mut needed);
        // In 8-byte units: the answer starts with a UNICODE_STRING, which holds a pointer.
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(2)];
        let status = NtQueryInformationProcess(process, PROCESS_COMMAND_LINE_INFORMATION, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut needed);
        CloseHandle(process);
        if status != 0 {
            return None;
        }
        let text = &*(buffer.as_ptr() as *const UNICODE_STRING);
        if text.Buffer.is_null() {
            return None;
        }
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(text.Buffer, text.Length as usize / 2)))
    }
}

#[derive(Clone, Debug)]
pub struct Window {
    pub hwnd: isize,
    pub pid: u32,
    pub class: String,
    pub title: String,
    pub width: i32,
    pub height: i32,
    pub is_hidden: bool,
    /// Where the window's frame starts on the screen.
    pub screen_left: i32,
    pub screen_top: i32,
}

impl Window {
    /// Helpers a process owns on a hidden desktop that are not what anyone wants to look at: the input indicator
    /// Windows adds, and ESET's browser frame.
    fn is_helper(&self) -> bool {
        const HELPER_CLASSES: [&str; 3] = ["UAC_InputIndicatorOverlayWnd", "UAC Input Indicator", "ESET_OPP_Frame"];
        self.width <= 0 || self.height <= 0 || HELPER_CLASSES.contains(&self.class.as_str())
    }
}

/// Window enumeration callback: `list` points at the `Vec<isize>` that collects the handles.
///
/// # Safety
/// `list` must point at a live `Vec<isize>` for the whole enumeration.
pub unsafe extern "system" fn collect_handle(hwnd: HWND, list: LPARAM) -> i32 {
    unsafe { (*(list as *mut Vec<isize>)).push(hwnd as isize) };
    TRUE
}

/// The windows on a desktop someone could want to see: visible, with a size, not one of the helpers.
pub fn windows_on(desktop: &Desktop) -> Vec<Window> {
    let mut handles: Vec<isize> = Vec::new();
    // Its result is not an error signal here: a desktop with no windows reports false too, and an empty list says so.
    unsafe { EnumDesktopWindows(desktop.handle, Some(collect_handle), &mut handles as *mut Vec<isize> as LPARAM) };
    handles
        .into_iter()
        .filter_map(|handle| {
            let hwnd = handle as HWND;
            if unsafe { IsWindowVisible(hwnd) } == 0 {
                return None;
            }
            let mut pid = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
            let mut class = [0u16; TEXT_BUFFER];
            unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) };
            let mut title = [0u16; TEXT_BUFFER];
            unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
            let mut rect: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(hwnd, &mut rect) };
            let mut frame: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetWindowRect(hwnd, &mut frame) };
            let window = Window {
                hwnd: handle,
                pid,
                class: text_of(&class),
                title: text_of(&title),
                width: rect.right - rect.left,
                height: rect.bottom - rect.top,
                is_hidden: desktop.is_hidden,
                screen_left: frame.left,
                screen_top: frame.top,
            };
            (!window.is_helper()).then_some(window)
        })
        .collect()
}

/// Everything running out of the user's sight: the hidden desktop's windows and those on the virtual monitor. Never any
/// other window on the real desktop, which could be the user's.
pub fn background_windows() -> io::Result<Vec<Window>> {
    let mut windows = windows_on(&Desktop::hidden()?);
    if let Some(bounds) = monitor::current_bounds() {
        windows.extend(windows_on(&Desktop::real()?).into_iter().filter(|window| monitor::holds(&bounds, window)));
    }
    Ok(windows)
}

/// Whether the tray app, and so the sound guard, is running.
pub fn tray_running() -> bool {
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    let name = wide(TRAY_MUTEX);
    let mutex = unsafe { OpenMutexW(SYNCHRONIZE_ACCESS, 0, name.as_ptr()) };
    if mutex.is_null() {
        return false;
    }
    unsafe { CloseHandle(mutex) };
    true
}

/// Reads a value from another process; None when Windows will not give all of it.
unsafe fn read_from<T: Copy>(process: HANDLE, address: usize) -> Option<T> {
    let mut value = std::mem::MaybeUninit::<T>::uninit();
    let mut read = 0usize;
    let ok = unsafe { ReadProcessMemory(process, address as *const c_void, value.as_mut_ptr().cast(), size_of::<T>(), &mut read) };
    (ok != 0 && read == size_of::<T>()).then(|| unsafe { value.assume_init() })
}

/// Reads from another process's parameters, RTL_USER_PROCESS_PARAMETERS, at their address in it. The offsets are the 64-bit PEB's and
/// parameters', which also exist for a 32-bit program (tested on one).
fn from_parameters<T>(pid: u32, read: impl FnOnce(HANDLE, usize) -> Option<T>) -> Option<T> {
    const PEB_PROCESS_PARAMETERS: usize = 0x20;
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, 0, pid);
        if process.is_null() {
            return None;
        }
        let found = (|| {
            let mut basic: PROCESS_BASIC_INFORMATION = std::mem::zeroed();
            let status = NtQueryInformationProcess(
                process,
                ProcessBasicInformation,
                (&mut basic as *mut PROCESS_BASIC_INFORMATION).cast(),
                size_of::<PROCESS_BASIC_INFORMATION>() as u32,
                null_mut(),
            );
            if status != 0 {
                return None;
            }
            let parameters: usize = read_from(process, basic.PebBaseAddress as usize + PEB_PROCESS_PARAMETERS)?;
            read(process, parameters)
        })();
        CloseHandle(process);
        found
    }
}

/// Reads so many UTF-16 units from another process; None when Windows will not give all of them.
fn units_from(process: HANDLE, address: usize, units: usize) -> Option<Vec<u16>> {
    let mut text = vec![0u16; units];
    let mut read = 0usize;
    let ok = unsafe { ReadProcessMemory(process, address as *const c_void, text.as_mut_ptr().cast(), units * 2, &mut read) };
    (ok != 0 && read == units * 2).then_some(text)
}

/// The desktop a process belongs to, as Windows records it in the process's parameters: the name it was started with, or
/// the one its parent had. It is the only record for a program with no window, and no Windows function returns it. If the
/// offsets ever move this reads nothing, and tools\verify-sound.ps1 fails.
fn desktop_of(pid: u32) -> Option<String> {
    const PARAMETERS_DESKTOP_INFO: usize = 0xC0;
    from_parameters(pid, |process, parameters| {
        // A UNICODE_STRING: its length in bytes, then its address at the next 8-byte boundary.
        let [length, address]: [usize; 2] = unsafe { read_from(process, parameters + PARAMETERS_DESKTOP_INFO) }?;
        let units = (length & 0xFFFF) / 2;
        if units == 0 || units > TEXT_BUFFER {
            return None;
        }
        Some(OsString::from_wide(&units_from(process, address, units)?).to_string_lossy().into_owned())
    })
}

/// The environment Windows keeps for a process: a block of `NAME=value` strings, each ending in a zero, and one zero more. It is
/// what its parent gave it until the process changes it through Windows, which Git Bash does not: it keeps its own variables apart.
pub fn initial_environment_of(pid: u32) -> Option<Vec<u16>> {
    const PARAMETERS_ENVIRONMENT: usize = 0x80;
    const PARAMETERS_ENVIRONMENT_SIZE: usize = 0x3F0;
    /// Windows allows 32,767 characters a variable; no real environment comes near this.
    const LARGEST: usize = 64 * 1024 * 1024;
    from_parameters(pid, |process, parameters| {
        let address: usize = unsafe { read_from(process, parameters + PARAMETERS_ENVIRONMENT) }?;
        let bytes: usize = unsafe { read_from(process, parameters + PARAMETERS_ENVIRONMENT_SIZE) }?;
        if address == 0 || !(4..=LARGEST).contains(&bytes) {
            return None;
        }
        let mut block = units_from(process, address, bytes / 2)?;
        // Up to the two zeros that end it: the size can include room after them.
        let end = block.windows(2).position(|pair| pair == [0, 0])?;
        block.truncate(end + 2);
        Some(block)
    })
}

/// Whether a process runs on the hidden desktop, window or no window.
pub fn runs_hidden(pid: u32) -> bool {
    desktop_is_hidden(pid).unwrap_or(false)
}

/// The same, or None when Windows will not say, as for a process too new to read yet.
pub fn desktop_is_hidden(pid: u32) -> Option<bool> {
    desktop_of(pid).map(|name| name.eq_ignore_ascii_case(&hidden_desktop_path()))
}

/// The program name of a process, for listings; empty when Windows will not say.
pub fn process_name(pid: u32) -> String {
    let full = process_path(pid);
    Path::new(&full).file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or(full)
}

/// The full path of a process's program; empty when Windows will not say.
pub fn process_path(pid: u32) -> String {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return String::new();
        }
        let mut buffer = [0u16; TEXT_BUFFER];
        let mut length = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut length);
        CloseHandle(process);
        if ok == 0 {
            return String::new();
        }
        text_of(&buffer)
    }
}

pub struct Picture {
    pub width: u32,
    pub height: u32,
    /// Top-down, four bytes per pixel: blue, green, red, unused.
    pub bgra: Vec<u8>,
}

impl Picture {
    /// Fully black. An OpenGL window on a hidden desktop captures this way from outside; DirectX and ordinary windows
    /// do not.
    pub fn is_black(&self) -> bool {
        self.bgra.as_chunks::<4>().0.iter().all(|&[blue, green, red, _]| blue == 0 && green == 0 && red == 0)
    }

    pub fn save_png(&self, path: &Path) -> io::Result<()> {
        let file = std::fs::File::create(path)?;
        let mut encoder = png::Encoder::new(io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(io::Error::other)?;
        let rgb: Vec<u8> = self.bgra.as_chunks::<4>().0.iter().flat_map(|&[blue, green, red, _]| [red, green, blue]).collect();
        writer.write_image_data(&rgb).map_err(io::Error::other)?;
        writer.finish().map_err(io::Error::other)
    }
}

/// Runs `work` on a thread that has joined the hidden or the real desktop. Windows refuses a window's child list and its
/// messages to a thread on another desktop, so everything that touches a window goes through here. A new thread because
/// a thread can join a desktop only while it owns no windows.
pub fn on_desktop<T: Send>(hidden: bool, work: impl FnOnce() -> io::Result<T> + Send) -> io::Result<T> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let desktop = if hidden { Desktop::hidden()? } else { Desktop::real()? };
                let original = unsafe { GetThreadDesktop(GetCurrentThreadId()) };
                if unsafe { SetThreadDesktop(desktop.handle) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                let result = work();
                // Back where the thread started: Windows will not close a desktop handle a thread still uses, and the tray's
                // preview comes here every 700 ms.
                unsafe { SetThreadDesktop(original) };
                result
            })
            .join()
            .map_err(|_| io::Error::other("the thread for that desktop stopped"))?
    })
}

/// The window's own pixels, DirectX included, without bringing it forward.
pub fn capture(window: &Window) -> io::Result<Picture> {
    let hwnd = window.hwnd;
    let (width, height) = (window.width, window.height);
    on_desktop(window.is_hidden, || unsafe { print_window(hwnd as HWND, width, height) })
}

/// Posts a message to a window from its own desktop.
pub fn post_to_window(window: &Window, message: u32, wparam: usize, lparam: LPARAM) -> io::Result<()> {
    let hwnd = window.hwnd;
    on_desktop(window.is_hidden, || {
        if unsafe { PostMessageW(hwnd as HWND, message, wparam, lparam) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    })
}

unsafe fn print_window(hwnd: HWND, width: i32, height: i32) -> io::Result<Picture> {
    unsafe {
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = -height; // top-down
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits: *mut c_void = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let result = if bitmap.is_null() || bits.is_null() {
            Err(io::Error::last_os_error())
        } else {
            let previous = SelectObject(dc, bitmap);
            let printed = PrintWindow(hwnd, dc, PW_CLIENTONLY | PW_RENDERFULLCONTENT);
            let picture = if printed == 0 {
                Err(io::Error::last_os_error())
            } else {
                let size = width as usize * height as usize * 4;
                Ok(Picture {
                    width: width as u32,
                    height: height as u32,
                    bgra: std::slice::from_raw_parts(bits as *const u8, size).to_vec(),
                })
            };
            SelectObject(dc, previous);
            DeleteObject(bitmap);
            picture
        };
        DeleteDC(dc);
        ReleaseDC(null_mut(), screen);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tray_offers_each_open_program_once_and_none_already_listed() {
        let open: Vec<String> = ["Blender.exe", "blender.exe", "notepad.exe", "Code.exe", "explorer.exe", "quietdesk-tray.exe"]
            .into_iter()
            .map(String::from)
            .collect();
        let listed = vec!["notepad.exe".to_string()];
        assert_eq!(programs_to_offer(&open, &listed), ["blender.exe", "code.exe"]);
        assert!(programs_to_offer(&[], &listed).is_empty());
    }

    /// A program's main window is offered; a tool window or a window owned by another is not, as the taskbar shows them. On the
    /// hidden desktop, so nothing shows on anyone's screen.
    #[test]
    fn the_programs_offered_are_those_with_a_window_the_taskbar_would_show() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_EX_NOACTIVATE, WS_POPUP, WS_VISIBLE};
        let found = on_desktop(true, || {
            let (class, title) = (wide("STATIC"), wide("QuietDesk test window"));
            let make = |extended: u32, owner: HWND| unsafe {
                CreateWindowExW(
                    WS_EX_NOACTIVATE | extended, class.as_ptr(), title.as_ptr(), WS_POPUP | WS_VISIBLE,
                    -32000, -32000, 120, 80, owner, null_mut(), null_mut(), null(),
                )
            };
            let main = make(0, null_mut());
            let tool = make(WS_EX_TOOLWINDOW, null_mut());
            let owned = make(0, main);
            let windows = windows_on(&Desktop::hidden()?);
            let judged = |hwnd: HWND| windows.iter().find(|window| window.hwnd == hwnd as isize).map(is_app_window);
            let result = (judged(main), judged(tool), judged(owned), open_programs_on(&Desktop::hidden()?));
            unsafe {
                DestroyWindow(owned);
                DestroyWindow(tool);
                DestroyWindow(main);
            }
            Ok(result)
        })
        .unwrap();
        assert_eq!((found.0, found.1, found.2), (Some(true), Some(false), Some(false)));
        let this_program = program_name(&process_name(std::process::id())).unwrap();
        assert!(found.3.contains(&this_program), "{this_program} not offered among {:?}", found.3);
    }

    /// A held file once read as off, and the keeper unplugged the monitor with the mode on.
    #[test]
    fn a_mode_file_that_cannot_be_read_is_not_off() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("quietdesk-mode-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(mode_in(&dir), Ok(Mode::Off)), "no file means off");
        std::fs::write(dir.join("mode"), "on\n").unwrap();
        assert!(matches!(mode_in(&dir), Ok(Mode::On)));
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(dir.join("mode")).unwrap();
        assert!(mode_in(&dir).is_err(), "a file someone holds says nothing");
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A child started with a variable reads back with it, as its parent gave it.
    #[test]
    fn a_process_environment_reads_back_as_it_was_given() {
        use std::os::windows::process::CommandExt;
        let mut child = std::process::Command::new("ping.exe")
            .args(["-n", "3", "127.0.0.1"])
            .env("QUIETDESK_TEST_GIVEN", "a value with = and ü")
            .stdout(std::process::Stdio::null())
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let block = initial_environment_of(child.id());
        let _ = child.kill();
        let _ = child.wait();
        let block = block.expect("the environment of a child of ours can be read");
        assert_eq!(&block[block.len() - 2..], [0, 0], "the block ends as Windows ends one");
        let text = String::from_utf16_lossy(&block);
        let names: Vec<&str> = text.split('\0').collect();
        assert!(names.contains(&"QUIETDESK_TEST_GIVEN=a value with = and ü"), "{names:?}");
    }

    #[test]
    fn a_program_is_kept_by_its_file_name() {
        assert_eq!(program_name("blender").as_deref(), Some("blender.exe"));
        assert_eq!(program_name(" Blender.EXE ").as_deref(), Some("blender.exe"));
        assert_eq!(program_name(r#""C:\Program Files\Blender Foundation\Blender 5.2\blender.exe""#).as_deref(), Some("blender.exe"));
        assert_eq!(program_name("/c/tools/godot.cmd").as_deref(), Some("godot.cmd"));
        assert_eq!(program_name("  "), None);
    }

    /// The commands sessions really write, from Bash and from PowerShell, and the ones that only mention a similar name.
    #[test]
    fn a_command_names_a_program_by_its_word_or_the_end_of_its_path() {
        let list = vec!["blender.exe".to_string()];
        for named in [
            "blender.exe file.blend --python x.py",
            "blender --factory-startup &",
            r#"& "C:\Program Files\Blender Foundation\Blender 5.2\blender.exe" C:\x\a.blend"#,
            "'/c/Program Files/Blender Foundation/Blender 5.2/blender.exe' -b a.blend",
            "Start-Process -FilePath 'C:/Program Files/Blender Foundation/Blender 5.2/blender.exe'",
            "cd x; BLENDER a.blend",
        ] {
            assert!(names_a_program(named, &list), "missed: {named}");
        }
        for not_named in [
            "cd C:\\Users\\x\\BlenderProject && cargo test",
            "python blender_export.py",
            "echo blenderish",
            "ls /c/tools/blender-addons",
            "uvx blender-mcp",
        ] {
            assert!(!names_a_program(not_named, &list), "named by mistake: {not_named}");
        }
        assert!(!names_a_program("blender.exe a.blend", &[]), "an empty list names nothing");
    }

    fn utf16(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn the_first_token_ends_the_way_windows_ends_it() {
        assert_eq!(after_first_token(&utf16(r#"pwsh -a "b c""#)), utf16(r#" -a "b c""#));
        assert_eq!(after_first_token(&utf16(r#""C:\Program Files\x.exe"  -a"#)), utf16("  -a"));
        assert_eq!(after_first_token(&utf16(r#""C:\x y\z.exe""#)), utf16(""));
        assert_eq!(after_first_token(&utf16("alone")), utf16(""));
    }

    #[test]
    fn a_command_line_keeps_its_arguments_exactly() {
        let line = command_line_for(Path::new(r"C:\Program Files\x.exe"), &utf16(r#"  -a "b ^c" %d%"#));
        assert_eq!(line, utf16(r#""C:\Program Files\x.exe"  -a "b ^c" %d%"#));
        let glued = command_line_for(Path::new("x.exe"), &utf16("-a"));
        assert_eq!(glued, utf16(r#""x.exe" -a"#));
    }

    #[test]
    fn helpers_are_not_listed_but_real_windows_are() {
        let make = |class: &str, width| Window {
            hwnd: 1,
            pid: 1,
            class: class.into(),
            title: String::new(),
            width,
            height: 100,
            is_hidden: true,
            screen_left: 0,
            screen_top: 0,
        };
        assert!(make("UAC Input Indicator", 100).is_helper());
        assert!(make("Game Client", 0).is_helper());
        assert!(!make("Game Client", 100).is_helper());
    }
}
