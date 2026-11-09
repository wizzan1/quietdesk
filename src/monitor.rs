//! The virtual monitor, for what cannot run hidden: to Windows it is a real display, so windows on it are composed and
//! presented normally, yet no screen shows them.
//!
//! Plugging it in or out is what the user notices: Windows plays the connect sound and every screen freezes for a second, as
//! with a display cable (measured: it happened during the user's game, once per run). So while QuietDesk is on it stays
//! plugged in, from the moment the user turns the mode on, and no run plugs anything. Outside the mode it exists only while
//! something uses it.
//!
//! One keeper process owns the monitor. Each command gets a job of its own and a file saying so, and the keeper keeps every window
//! of its family on the monitor: the job's processes and everything they start. One job per command, because a job shared by
//! all can sit in one hierarchy only: once a process from outside any job has joined it, it refuses a process from inside another
//! job, and in the Claude app the Bash tool runs inside a job nothing may leave. A new job takes a process from anywhere. A job alone is
//! not enough, because the Store's PowerShell 7 lets its children out of any job, and parent links alone are not either, because
//! a launcher can exit before its child is seen. The keeper outlives the command that started it, because a game can outlive that
//! command too (a window left on an unplugged monitor would land on the user's screen). When the family has been gone for a
//! moment and the mode is off, the keeper unplugs the monitor. If the keeper dies, the driver unplugs it by itself about a second
//! later.
//!
//! The driver's interface is Parsec VDD's, as published in `core/parsec-vdd.h` of github.com/nomi-san/parsec-vdd.

use std::collections::HashMap;
use std::ffi::{OsStr, c_void};
use std::io;
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig, SDC_ALLOW_CHANGES, SDC_APPLY,
    DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, SDC_SAVE_TO_DATABASE, SDC_USE_SUPPLIED_DISPLAY_CONFIG, SetDisplayConfig,
};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_NOT_FOUND, ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING, FILETIME, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HWND,
    INVALID_HANDLE_VALUE, LPARAM, POINTL, RECT, TRUE, WAIT_OBJECT_0,
};
use windows_sys::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DISPLAY_DEVICE_ACTIVE, DISPLAYCONFIG_PATH_ACTIVE,
    DISPLAYCONFIG_PATH_MODE_IDX_INVALID, DISPLAY_DEVICE_PRIMARY_DEVICE,
    DISPLAY_DEVICEW, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH, DM_POSITION, ENUM_CURRENT_SETTINGS, EnumDisplayDevicesW,
    EnumDisplaySettingsW,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING, FILE_FLAG_OVERLAPPED, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ,
    FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::IO::{CancelIoEx, DeviceIoControl, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED};
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList, OpenJobObjectW, QueryInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, CREATE_SUSPENDED, CreateEventW, CreateMutexW, CreateProcessW, GetCurrentThreadId,
    GetProcessTimes, OpenMutexW, OpenProcess, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, ResetEvent, ResumeThread,
    STARTF_USEPOSITION, STARTF_USESHOWWINDOW, STARTUPINFOW, SetEvent, TerminateProcess, WaitForSingleObject,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowRect, GetWindowThreadProcessId, IsHungAppWindow, IsWindow, IsWindowVisible,
    SW_SHOWNOACTIVATE,
    SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetWindowPos,
};
use windows_sys::core::GUID;

use crate::{REAL_DESKTOP, Window, installed_state_dir, mode_is_certainly_off, note_problem, text_of, wide};

const ADAPTER_INTERFACE: GUID = GUID::from_u128(0x00b41627_04c4_429e_a26e_0265cf50c8fa);
const ADAPTER_NAME: &str = "Parsec Virtual Display Adapter";
const IOCTL_ADD: u32 = 0x0022_e004;
const IOCTL_REMOVE: u32 = 0x0022_a008;
const IOCTL_UPDATE: u32 = 0x0022_a00c;
const IOCTL_BUFFER: usize = 32;
const IOCTL_TIMEOUT_MS: u32 = 5000;

const KEEPER_MUTEX: &str = "Local\\QuietDeskMonitorKeeper";
const READY_EVENT: &str = "Local\\QuietDeskMonitorReady";
const COMMAND_JOB_PREFIX: &str = "Local\\QuietDeskMonitorCommand-";
pub const KEEPER_WORD: &str = "monitor-keeper";

const JOB_OBJECT_QUERY: u32 = 0x0004;
const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
/// The driver unplugs after about a second without one; its header asks for under 100 ms.
const LOOP_MS: u64 = 25;
const DISPLAY_WAIT: Duration = Duration::from_secs(5);
/// How long a new keeper waits for a monitor an earlier keeper left. Measured after a kill: one was gone in 1.2 s, two in 4.2 s.
const LEFTOVER_WAIT: Duration = Duration::from_secs(8);
const READY_WAIT_MS: u32 = 15_000;
/// How long the monitor stays after the last command is gone: sequential commands of one run share it.
const IDLE_BEFORE_UNPLUG: Duration = Duration::from_secs(3);
/// How long a new keeper waits for the command that started it.
const FIRST_COMMAND_WAIT: Duration = Duration::from_secs(20);
const MAX_JOB_PROCESSES: usize = 1024;
/// Smaller windows are helpers nobody would notice.
const SMALLEST_WINDOW: i32 = 100;
/// Windows are staggered so one does not fully cover the next: Windows treats a covered window as not visible.
const STAGGER: i32 = 48;
const STAGGER_SLOTS: i32 = 8;
const MOVE_RETRY: Duration = Duration::from_millis(100);
/// Every second loop: a process table costs more than the rest of the loop together.
const FAMILY_EVERY_LOOPS: u32 = 2;
/// How often a keeper with nothing to do asks whether the mode is still on: about once a second.
const MODE_EVERY_LOOPS: u32 = 40;
/// So many of those looks in a row without the display, and it is gone: a change of the user's screens can hide it for a moment.
const LOOKS_BEFORE_GONE: u32 = 3;
/// What the keeper writes in a command's file once it follows the job.
const KEEPER_FOLLOWS: &[u8] = b"followed";
/// How long a command waits, not yet running, for that answer. Measured with the screen free: a whole command took a median of 70 ms
/// against 57 with no monitor, at most 211. Only a keeper in trouble gets near this.
const KEEPER_ANSWER_WAIT: Duration = Duration::from_secs(2);
const KEEPER_ANSWER_LOOK: Duration = Duration::from_millis(5);
/// A change of the user's screens can switch the monitor off, or move it beside one of them. The keeper changes the layout back at
/// most so often in this time, so a tug of war with Windows, or with the user arranging their screens, cannot keep them flickering;
/// then it leaves things as they are.
const LAYOUT_CHANGES: usize = 3;
const LAYOUT_CHANGE_WINDOW: Duration = Duration::from_secs(60);
/// The screens can change between asking how many there are and listing them.
const LAYOUT_TRIES: u32 = 3;
/// How long a family member that exited can still be recognised as the parent of a process seen later.
const REMEMBER_PARENT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    /// Whether the two share more than one corner point: an edge, or any area. Across a shared edge the mouse moves from one to the
    /// other; across a corner point it does not.
    fn shares_more_than_a_corner(&self, other: &Bounds) -> bool {
        let across = self.right.min(other.right) - self.left.max(other.left);
        let down = self.bottom.min(other.bottom) - self.top.max(other.top);
        across >= 0 && down >= 0 && (across > 0 || down > 0)
    }
}

fn last_error() -> io::Error {
    io::Error::last_os_error()
}

struct Driver(HANDLE);

impl Driver {
    fn open() -> io::Result<Self> {
        unsafe {
            let set = SetupDiGetClassDevsW(&ADAPTER_INTERFACE, null(), null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
            if set as isize == -1 {
                return Err(last_error());
            }
            let mut interface: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
            interface.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
            let found = SetupDiEnumDeviceInterfaces(set, null(), &ADAPTER_INTERFACE, 0, &mut interface);
            let result = if found == 0 {
                Err(io::Error::other("the virtual display driver is not installed"))
            } else {
                let mut needed = 0u32;
                SetupDiGetDeviceInterfaceDetailW(set, &interface, null_mut(), 0, &mut needed, null_mut());
                // A DWORD size, then the device path. The size field means the fixed part: 8 on 64-bit Windows.
                let mut detail = vec![0u64; (needed as usize).div_ceil(8)];
                detail[0] = 8;
                if SetupDiGetDeviceInterfaceDetailW(set, &interface, detail.as_mut_ptr().cast(), needed, &mut needed, null_mut()) == 0 {
                    Err(last_error())
                } else {
                    let path = (detail.as_ptr() as *const u8).add(4) as *const u16;
                    let handle = CreateFileW(
                        path,
                        GENERIC_READ | GENERIC_WRITE,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        null(),
                        OPEN_EXISTING,
                        FILE_ATTRIBUTE_NORMAL | FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED | FILE_FLAG_WRITE_THROUGH,
                        null_mut(),
                    );
                    if handle == INVALID_HANDLE_VALUE { Err(last_error()) } else { Ok(Driver(handle)) }
                }
            };
            SetupDiDestroyDeviceInfoList(set);
            result
        }
    }

    fn control(&self, code: u32, input: &[u8]) -> io::Result<u32> {
        let mut buffer = [0u8; IOCTL_BUFFER];
        buffer[..input.len()].copy_from_slice(input);
        let mut output = 0u32;
        let mut transferred = 0u32;
        unsafe {
            let mut overlapped: OVERLAPPED = std::mem::zeroed();
            overlapped.hEvent = CreateEventW(null(), TRUE, 0, null());
            if overlapped.hEvent.is_null() {
                return Err(last_error());
            }
            let issued = DeviceIoControl(
                self.0,
                code,
                buffer.as_ptr().cast(),
                IOCTL_BUFFER as u32,
                (&mut output as *mut u32).cast::<c_void>(),
                size_of::<u32>() as u32,
                null_mut(),
                &mut overlapped,
            );
            if issued == 0 && GetLastError() != ERROR_IO_PENDING {
                let error = last_error();
                CloseHandle(overlapped.hEvent);
                return Err(error);
            }
            let finished = GetOverlappedResultEx(self.0, &overlapped, &mut transferred, IOCTL_TIMEOUT_MS, 0);
            let error = last_error();
            if finished == 0 {
                // Still running: the driver would write into `output` and `overlapped` on this stack after we return. It is
                // cancelled, and waited for, first.
                CancelIoEx(self.0, &overlapped);
                GetOverlappedResult(self.0, &overlapped, &mut transferred, TRUE);
            }
            CloseHandle(overlapped.hEvent);
            if finished == 0 { Err(error) } else { Ok(output) }
        }
    }

    fn ping(&self) -> io::Result<()> {
        self.control(IOCTL_UPDATE, &[]).map(|_| ())
    }

    fn add(&self) -> io::Result<u32> {
        let index = self.control(IOCTL_ADD, &[])?;
        self.ping()?;
        Ok(index)
    }

    fn remove(&self, index: u32) -> io::Result<()> {
        // A 16-bit big-endian index.
        self.control(IOCTL_REMOVE, &[(index >> 8) as u8, index as u8])?;
        self.ping()
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

struct Display {
    name: [u16; 32],
    is_virtual: bool,
    is_primary: bool,
}

fn active_displays() -> Vec<Display> {
    let mut displays = Vec::new();
    let mut index = 0;
    loop {
        let mut device: DISPLAY_DEVICEW = unsafe { std::mem::zeroed() };
        device.cb = size_of::<DISPLAY_DEVICEW>() as u32;
        if unsafe { EnumDisplayDevicesW(null(), index, &mut device, 0) } == 0 {
            return displays;
        }
        index += 1;
        if device.StateFlags & DISPLAY_DEVICE_ACTIVE != 0 {
            displays.push(Display {
                name: device.DeviceName,
                is_virtual: text_of(&device.DeviceString) == ADAPTER_NAME,
                is_primary: device.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
            });
        }
    }
}

/// How many virtual monitors Windows has now. One keeper holds one; a second is a leftover the user's mouse can wander onto.
pub fn virtual_display_count() -> usize {
    active_displays().iter().filter(|display| display.is_virtual).count()
}

fn current_mode(display: &Display) -> Option<DEVMODEW> {
    let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
    mode.dmSize = size_of::<DEVMODEW>() as u16;
    (unsafe { EnumDisplaySettingsW(display.name.as_ptr(), ENUM_CURRENT_SETTINGS, &mut mode) } != 0).then_some(mode)
}

fn bounds_of(display: &Display) -> Option<Bounds> {
    let mode = current_mode(display)?;
    let position = unsafe { mode.Anonymous1.Anonymous2.dmPosition };
    Some(Bounds {
        left: position.x,
        top: position.y,
        right: position.x + mode.dmPelsWidth as i32,
        bottom: position.y + mode.dmPelsHeight as i32,
    })
}

/// The monitor's mode at the main screen's size and colour depth with the nearest refresh rate. None if it has no such size.
fn mode_like(monitor: &Display, main: &DEVMODEW) -> Option<DEVMODEW> {
    let distance = |mode: &DEVMODEW| mode.dmDisplayFrequency.abs_diff(main.dmDisplayFrequency);
    let mut best: Option<DEVMODEW> = None;
    let mut index = 0;
    loop {
        let mut mode: DEVMODEW = unsafe { std::mem::zeroed() };
        mode.dmSize = size_of::<DEVMODEW>() as u16;
        if unsafe { EnumDisplaySettingsW(monitor.name.as_ptr(), index, &mut mode) } == 0 {
            return best;
        }
        index += 1;
        let same_size = mode.dmPelsWidth == main.dmPelsWidth && mode.dmPelsHeight == main.dmPelsHeight && mode.dmBitsPerPel == main.dmBitsPerPel;
        if same_size && best.as_ref().is_none_or(|other| distance(&mode) < distance(other)) {
            best = Some(mode);
        }
    }
}

/// Where a display of this size touches the user's screens at a single corner point and shares no edge with any of them, so the
/// mouse cannot wander onto it: the main screen's four corners first, then the other screens'. None when every corner is taken.
fn free_corner(main: Bounds, screens: &[Bounds], width: i32, height: i32) -> Option<(i32, i32)> {
    std::iter::once(main)
        .chain(screens.iter().copied())
        .flat_map(|s| [(s.right, s.bottom), (s.left - width, s.bottom), (s.right, s.top - height), (s.left - width, s.top - height)])
        .find(|&(left, top)| {
            let spot = Bounds { left, top, right: left + width, bottom: top + height };
            !screens.iter().any(|screen| spot.shares_more_than_a_corner(screen))
        })
}

/// Gives the monitor the main screen's size and the nearest refresh rate it has, so a program on it sees the display it
/// would see on the real screen, and places it at a corner where the user's mouse cannot wander onto it.
fn set_up_like_the_main_screen(monitor: &Display) {
    let displays = active_displays();
    // Windows can make the monitor the main screen as it comes: there is nothing to copy then, and `show_beside` puts the user's
    // own main screen back.
    let Some(main) = displays.iter().find(|d| d.is_primary && !d.is_virtual) else { return };
    let (Some(main_bounds), Some(main_mode), Some(current)) = (bounds_of(main), current_mode(main), current_mode(monitor)) else { return };
    let mut mode = mode_like(monitor, &main_mode).unwrap_or(current);
    let (width, height) = (mode.dmPelsWidth as i32, mode.dmPelsHeight as i32);
    let screens: Vec<Bounds> = displays.iter().filter(|d| !d.is_virtual).filter_map(bounds_of).collect();
    mode.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
    match free_corner(main_bounds, &screens, width, height) {
        Some((left, top)) => {
            mode.dmFields |= DM_POSITION;
            mode.Anonymous1.Anonymous2.dmPosition.x = left;
            mode.Anonymous1.Anonymous2.dmPosition.y = top;
        }
        None => note_problem("no corner beside your screens is free for the virtual monitor, so it stays where Windows put it, where your mouse may reach it"),
    }
    let result = unsafe { ChangeDisplaySettingsExW(monitor.name.as_ptr(), &mode, null_mut(), 0, null()) };
    if result != DISP_CHANGE_SUCCESSFUL {
        note_problem(&format!("could not set up the virtual monitor like the main screen: code {result}"));
    }
}

/// What Windows shows: its active paths, each a source (a screen in the user's layout) on a target (a display), and their modes.
#[derive(Clone)]
struct Layout {
    paths: Vec<DISPLAYCONFIG_PATH_INFO>,
    modes: Vec<DISPLAYCONFIG_MODE_INFO>,
}

impl Layout {
    fn now() -> io::Result<Self> {
        for _ in 0..LAYOUT_TRIES {
            let (mut path_count, mut mode_count) = (0u32, 0u32);
            let error = unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) };
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
            let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![unsafe { std::mem::zeroed() }; path_count as usize];
            let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![unsafe { std::mem::zeroed() }; mode_count as usize];
            let error = unsafe {
                QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut path_count, paths.as_mut_ptr(), &mut mode_count, modes.as_mut_ptr(), null_mut())
            };
            if error == ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            if error != 0 {
                return Err(io::Error::from_raw_os_error(error as i32));
            }
            paths.truncate(path_count as usize);
            modes.truncate(mode_count as usize);
            return Ok(Self { paths, modes });
        }
        Err(io::Error::other("the screens kept changing while Windows listed them"))
    }

    /// Which of its paths shows this screen, by the name `EnumDisplayDevicesW` gives it (`\\.\DISPLAY1`).
    fn path_of(&self, screen: &str) -> Option<usize> {
        self.paths.iter().position(|path| source_name(path).as_deref() == Some(screen))
    }

    /// Only the paths this keeps, with the modes they use; paths that shared a mode still share it.
    fn keeping(&self, keep: impl Fn(&DISPLAYCONFIG_PATH_INFO) -> bool) -> Layout {
        let mut kept = Layout { paths: Vec::new(), modes: Vec::new() };
        let mut moved: HashMap<u32, u32> = HashMap::new();
        for path in self.paths.iter().filter(|path| keep(path)) {
            let mut path = *path;
            for index in unsafe { [&mut path.sourceInfo.Anonymous.modeInfoIdx, &mut path.targetInfo.Anonymous.modeInfoIdx] } {
                *index = match self.modes.get(*index as usize) {
                    Some(mode) => *moved.entry(*index).or_insert_with(|| {
                        kept.modes.push(*mode);
                        (kept.modes.len() - 1) as u32
                    }),
                    None => DISPLAYCONFIG_PATH_MODE_IDX_INVALID,
                };
            }
            kept.paths.push(path);
        }
        kept
    }

    /// Adds a path of another layout, with the modes it uses, and returns where it is. Queried without QDC_VIRTUAL_MODE_AWARE, as
    /// here, a path's mode numbers are plain indexes into its layout's modes.
    fn add_from(&mut self, other: &Layout, path: &DISPLAYCONFIG_PATH_INFO) -> usize {
        let mut path = *path;
        for index in unsafe { [&mut path.sourceInfo.Anonymous.modeInfoIdx, &mut path.targetInfo.Anonymous.modeInfoIdx] } {
            *index = match other.modes.get(*index as usize) {
                Some(mode) => {
                    self.modes.push(*mode);
                    (self.modes.len() - 1) as u32
                }
                None => DISPLAYCONFIG_PATH_MODE_IDX_INVALID,
            };
        }
        self.paths.push(path);
        self.paths.len() - 1
    }

    /// Each screen's place in the user's layout, by the index of its source mode.
    fn screens(&self) -> Vec<(usize, Bounds)> {
        self.modes
            .iter()
            .enumerate()
            .filter(|(_, mode)| mode.infoType == DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE)
            .map(|(index, mode)| {
                let source = unsafe { mode.Anonymous.sourceMode };
                let (left, top) = (source.position.x, source.position.y);
                (index, Bounds { left, top, right: left + source.width as i32, bottom: top + source.height as i32 })
            })
            .collect()
    }

    /// Puts the screen this path shows, the monitor's, at a corner of all the others where the user's mouse cannot cross onto it,
    /// unless it is at one already. An error when no corner is free, when the monitor is the main screen or none of the others is,
    /// or when it shows a copy of another screen: a layout like that must never be kept.
    fn place_at_a_corner(&mut self, path: usize) -> io::Result<()> {
        let source_mode = |path: &DISPLAYCONFIG_PATH_INFO| unsafe { path.sourceInfo.Anonymous.modeInfoIdx } as usize;
        let mode = source_mode(&self.paths[path]);
        if self.paths.iter().filter(|other| source_mode(other) == mode).count() > 1 {
            return Err(io::Error::other("the monitor shows a copy of one of your screens"));
        }
        let screens = self.screens();
        let monitor = screens.iter().find(|(index, _)| *index == mode).map(|(_, bounds)| *bounds);
        let monitor = monitor.ok_or_else(|| io::Error::other("the monitor has no place among your screens"))?;
        let theirs: Vec<Bounds> = screens.iter().filter(|(index, _)| *index != mode).map(|(_, bounds)| *bounds).collect();
        let main = theirs.iter().find(|screen| (screen.left, screen.top) == (0, 0));
        let main = *main.ok_or_else(|| io::Error::other("none of your own screens is the main screen"))?;
        if apart(&monitor, &theirs) {
            return Ok(());
        }
        let (width, height) = (monitor.right - monitor.left, monitor.bottom - monitor.top);
        let (x, y) = free_corner(main, &theirs, width, height).ok_or_else(|| io::Error::other("no corner beside your screens is free"))?;
        self.modes[mode].Anonymous.sourceMode.position = POINTL { x, y };
        Ok(())
    }

    /// Has Windows show exactly this, and keep it as its choice for this set of screens: the next time it sees the same set, it
    /// shows it at once.
    fn apply(&self) -> io::Result<()> {
        let flags = SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_SAVE_TO_DATABASE | SDC_ALLOW_CHANGES;
        let error = unsafe { SetDisplayConfig(self.paths.len() as u32, self.paths.as_ptr(), self.modes.len() as u32, self.modes.as_ptr(), flags) };
        if error == 0 { Ok(()) } else { Err(io::Error::from_raw_os_error(error)) }
    }
}

/// Whether a display here is out of the way of the user's mouse: not the main screen, and sharing no more than a corner point with
/// any of their screens.
fn apart(spot: &Bounds, screens: &[Bounds]) -> bool {
    (spot.left, spot.top) != (0, 0) && !screens.iter().any(|screen| spot.shares_more_than_a_corner(screen))
}

fn source_name(path: &DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut name: DISPLAYCONFIG_SOURCE_DEVICE_NAME = unsafe { std::mem::zeroed() };
    name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    name.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
    name.header.adapterId = path.sourceInfo.adapterId;
    name.header.id = path.sourceInfo.id;
    (unsafe { DisplayConfigGetDeviceInfo(&mut name.header) } == 0).then(|| text_of(&name.viewGdiDeviceName))
}

/// Shows the user's screens exactly as they had them before the monitor came (`before`), with the monitor at a corner beside them,
/// and has Windows keep that. Plugging the monitor in makes Windows apply what it last chose for the new set of screens: that can
/// light up a screen the user had turned off, or move one, and with "PC screen only" Windows switches everything else off again a
/// moment later, the monitor too. Returns the monitor's path.
fn show_beside(before: &Layout, monitor: &Display) -> io::Result<DISPLAYCONFIG_PATH_INFO> {
    let now = Layout::now()?;
    let monitor_path = now.path_of(&text_of(&monitor.name)).ok_or_else(|| io::Error::other("Windows does not show the monitor"))?;
    let monitor_path = now.paths[monitor_path];
    // The user's screens are never on the monitor's adapter. A path there is a monitor an earlier keeper left, read while Windows
    // was still taking it away: kept, it would be saved as part of the user's layout.
    let adapter = |path: &DISPLAYCONFIG_PATH_INFO| (path.targetInfo.adapterId.LowPart, path.targetInfo.adapterId.HighPart);
    let mut layout = before.keeping(|path| adapter(path) != adapter(&monitor_path));
    // Never a layout without one of the user's own screens.
    if layout.paths.is_empty() {
        return Err(io::Error::other("Windows showed none of your own screens"));
    }
    let added = layout.add_from(&now, &monitor_path);
    layout.place_at_a_corner(added)?;
    layout.apply()?;
    Ok(monitor_path)
}

/// Puts the monitor back at a corner when a change of the user's screens has moved it beside one of them.
fn move_to_a_corner(monitor: &Display) -> io::Result<()> {
    let mut layout = Layout::now()?;
    let path = layout.path_of(&text_of(&monitor.name)).ok_or_else(|| io::Error::other("Windows does not show the monitor"))?;
    layout.place_at_a_corner(path)?;
    layout.apply()
}

/// Turns the monitor's path on again beside what Windows shows now, after Windows switched it off. Unlike plugging it in anew, this
/// plays no connect sound and freezes no screen.
fn show_again(monitor_path: &DISPLAYCONFIG_PATH_INFO) -> io::Result<()> {
    let mut layout = Layout::now()?;
    if layout.paths.is_empty() {
        return Err(io::Error::other("Windows shows no screen at all"));
    }
    let mut path = *monitor_path;
    path.flags |= DISPLAYCONFIG_PATH_ACTIVE;
    path.sourceInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
    layout.paths.push(path);
    layout.apply()
}

/// Sizes and places the monitor, then has Windows show it beside the user's screens as they were `before` it came. Where it is, and
/// its path to show it again by.
fn settle(monitor: &Display, before: Option<&Layout>) -> Option<(Bounds, Option<DISPLAYCONFIG_PATH_INFO>)> {
    set_up_like_the_main_screen(monitor);
    let before = before.ok_or_else(|| io::Error::other("which screens Windows showed before it came is not known"));
    let path = match before.and_then(|before| show_beside(before, monitor)) {
        Ok(path) => Some(path),
        Err(error) => {
            note_problem(&format!("could not have Windows keep the virtual monitor beside your screens: {error}"));
            None
        }
    };
    Some((bounds_of(monitor)?, path))
}

/// Written aside, then put in place in one step: a reader never sees an empty file, which would read as no monitor.
fn record_bounds(bounds: &Bounds) {
    let aside = bounds_file().with_extension("new");
    if let Err(error) = std::fs::create_dir_all(installed_state_dir())
        .and_then(|()| std::fs::write(&aside, format!("{} {} {} {}", bounds.left, bounds.top, bounds.right, bounds.bottom)))
        .and_then(|()| std::fs::rename(&aside, bounds_file()))
    {
        note_problem(&format!("could not record where the monitor is: {error}"));
    }
}

/// In the installed folder even with a test's own: one keeper holds the monitor for this Windows session, and a test that looked
/// elsewhere would not find a monitor that is plugged in, and would run on the real screen.
fn bounds_file() -> PathBuf {
    installed_state_dir().join("monitor")
}

/// Whether a virtual monitor can be had at all.
pub fn driver_installed() -> bool {
    Driver::open().is_ok()
}

/// Where the monitor is, while a keeper runs.
pub fn current_bounds() -> Option<Bounds> {
    let name = wide(KEEPER_MUTEX);
    let mutex = unsafe { OpenMutexW(SYNCHRONIZE_ACCESS, 0, name.as_ptr()) };
    if mutex.is_null() {
        return None;
    }
    unsafe { CloseHandle(mutex) };
    let text = std::fs::read_to_string(bounds_file()).ok()?;
    let values: Vec<i32> = text.split_whitespace().filter_map(|word| word.parse().ok()).collect();
    match values.as_slice() {
        [left, top, right, bottom] => Some(Bounds { left: *left, top: *top, right: *right, bottom: *bottom }),
        _ => None,
    }
}

/// Is this window on the virtual monitor, and therefore a test's, not the user's? Windows also puts a taskbar there.
pub fn holds(bounds: &Bounds, window: &Window) -> bool {
    bounds.contains(window.screen_left, window.screen_top) && !crate::process_name(window.pid).eq_ignore_ascii_case("explorer.exe")
}

#[repr(C)]
struct ProcessIdList {
    assigned: u32,
    listed: u32,
    ids: [usize; MAX_JOB_PROCESSES],
}

/// None when Windows will not say: that must never read as an empty job, or a running test would lose its monitor.
fn processes_in(job: HANDLE) -> Option<Vec<u32>> {
    let mut list: Box<ProcessIdList> = Box::new(unsafe { std::mem::zeroed() });
    let ok = unsafe {
        QueryInformationJobObject(
            job,
            JobObjectBasicProcessIdList,
            (&mut *list as *mut ProcessIdList).cast(),
            size_of::<ProcessIdList>() as u32,
            null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }
    Some(list.ids[..list.listed as usize].iter().map(|&id| id as u32).collect())
}

/// One file per command on the monitor, named after its first process: how the keeper and the tray find the commands' jobs. In the
/// installed folder, as `bounds_file` is.
fn command_jobs_folder() -> PathBuf {
    installed_state_dir().join("monitor-commands")
}

fn command_job_name(file_name: &OsStr) -> Vec<u16> {
    wide(format!("{COMMAND_JOB_PREFIX}{}", file_name.to_string_lossy()))
}

fn files_in(folder: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(folder).map(|entries| entries.flatten().map(|entry| entry.path()).collect()).unwrap_or_default()
}

/// The processes in the jobs of the commands on the monitor, for the tray. Only the keeper tidies the folder.
pub(crate) fn command_processes() -> Vec<u32> {
    let mut processes = Vec::new();
    for file in files_in(&command_jobs_folder()) {
        let Some(file_name) = file.file_name() else { continue };
        let name = command_job_name(file_name);
        let job = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) };
        if !job.is_null() {
            processes.extend(processes_in(job).unwrap_or_default());
            unsafe { CloseHandle(job) };
        }
    }
    processes
}

/// The keeper's hold on the commands' jobs. Holding one keeps its name while the command has gone and what it started has not.
struct CommandJobs {
    folder: PathBuf,
    open: HashMap<PathBuf, HANDLE>,
}

impl CommandJobs {
    fn new(folder: PathBuf) -> Self {
        Self { folder, open: HashMap::new() }
    }

    /// The processes in all of them. None when Windows will not say for one: that must never read as empty, or a running test
    /// would lose its monitor.
    fn processes(&mut self) -> Option<Vec<u32>> {
        for file in files_in(&self.folder) {
            if self.open.contains_key(&file) {
                continue;
            }
            let Some(file_name) = file.file_name() else { continue };
            let name = command_job_name(file_name);
            let job = unsafe { OpenJobObjectW(JOB_OBJECT_QUERY, 0, name.as_ptr()) };
            if job.is_null() {
                // No such job: its command was killed before anyone held it. Any other refusal leaves the file alone.
                if unsafe { GetLastError() } == ERROR_FILE_NOT_FOUND {
                    let _ = std::fs::remove_file(&file);
                }
                continue;
            }
            if let Err(error) = std::fs::write(&file, KEEPER_FOLLOWS) {
                note_problem(&format!("could not tell a command that its job is followed: {error}"));
            }
            self.open.insert(file, job);
        }
        let mut all = Some(Vec::new());
        self.open.retain(|file, job| match processes_in(*job) {
            Some(processes) if processes.is_empty() => {
                unsafe { CloseHandle(*job) };
                let _ = std::fs::remove_file(file);
                false
            }
            Some(processes) => {
                if let Some(all) = &mut all {
                    all.extend(processes);
                }
                true
            }
            None => {
                all = None;
                true
            }
        });
        all
    }
}

/// When a process started, in Windows' clock. With its id this names one process: ids are reused.
pub(crate) fn started_at(pid: u32) -> Option<u64> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let ok = GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user);
        CloseHandle(process);
        (ok != 0).then(|| (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }
}

/// Every running process as (id, parent id).
pub(crate) fn process_table() -> Option<Vec<(u32, u32)>> {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut table = Vec::new();
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut more = Process32FirstW(snapshot, &mut entry) != 0;
        while more {
            table.push((entry.th32ProcessID, entry.th32ParentProcessID));
            more = Process32NextW(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        Some(table)
    }
}

/// The processes of the commands on the monitor. A process counts only if it started after its parent and its start time
/// still matches: a reused id must never make the keeper take hold of one of the user's windows.
#[derive(Default)]
struct Family {
    live: HashMap<u32, u64>,
    /// Members that ended: their start, when they were seen gone (in the same clock as start times), and since when.
    gone: HashMap<u32, (u64, u64, Instant)>,
}

/// Now, in the clock of process start times.
fn now_as_filetime() -> u64 {
    let mut now = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    (u64::from(now.dwHighDateTime) << 32) | u64::from(now.dwLowDateTime)
}

impl Family {
    fn refresh(&mut self, in_job: &[u32]) {
        // Nothing to follow: the keeper can idle for days, and the process table is the costly part of its loop.
        if in_job.is_empty() && self.live.is_empty() && self.gone.is_empty() {
            return;
        }
        let Some(table) = process_table() else { return };
        let own = std::process::id();

        let live: Vec<(u32, u64)> = self.live.iter().map(|(&pid, &at)| (pid, at)).collect();
        for (pid, at) in live {
            if started_at(pid) != Some(at) {
                self.live.remove(&pid);
                self.gone.insert(pid, (at, now_as_filetime(), Instant::now()));
            }
        }
        self.gone.retain(|_, (_, _, since)| since.elapsed() < REMEMBER_PARENT);

        for &pid in in_job {
            if pid != own && !self.live.contains_key(&pid) && let Some(at) = started_at(pid) {
                self.live.insert(pid, at);
            }
        }
        // More than one pass: a child and its own child can appear in the same table, in any order.
        loop {
            let mut added = false;
            for &(pid, parent) in &table {
                if pid == own || self.live.contains_key(&pid) {
                    continue;
                }
                // A member that ended can only be the parent of a process started before it was seen gone, and before
                // whatever holds its id now: Windows reuses ids, and the new holder's children are not the family's.
                let parent_known = self.live.get(&parent).map(|&at| (at, None)).or_else(|| self.gone.get(&parent).map(|&(at, gone_at, _)| (at, Some(gone_at))));
                if let Some((parent_at, gone_at)) = parent_known
                    && let Some(at) = started_at(pid)
                    && at >= parent_at
                    && gone_at.is_none_or(|gone_at| at <= gone_at && started_at(parent).is_none_or(|holder| at < holder))
                {
                    self.live.insert(pid, at);
                    added = true;
                }
            }
            if !added {
                break;
            }
        }
    }

    fn members(&self) -> Vec<u32> {
        self.live.keys().copied().collect()
    }
}

/// Gives the focus back when a test's program has taken it: nothing can stop a program taking it as it starts, and the user
/// may be in a game. Windows refuses this to a background process unless it shares input with the current foreground thread
/// for the moment of the call. A command that needs the focus itself belongs on `quietdesk visible`, not on the monitor.
#[derive(Default)]
struct FocusGuard {
    owners_window: Option<isize>,
}

impl FocusGuard {
    fn check(&mut self, processes: &[u32]) {
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_null() {
            return;
        }
        let mut owner = 0;
        let foreground_thread = unsafe { GetWindowThreadProcessId(foreground, &mut owner) };
        if !processes.contains(&owner) {
            self.owners_window = Some(foreground as isize);
            return;
        }
        let Some(window) = self.owners_window else { return };
        unsafe {
            // Sharing input with a hung window's thread can hang this one too.
            if IsWindow(window as HWND) == 0 || IsHungAppWindow(foreground) != 0 {
                return;
            }
            let this_thread = GetCurrentThreadId();
            let attached = AttachThreadInput(this_thread, foreground_thread, TRUE) != 0;
            SetForegroundWindow(window as HWND);
            if attached {
                AttachThreadInput(this_thread, foreground_thread, 0);
            }
        }
    }
}

#[derive(Default)]
struct Herd {
    slots: HashMap<isize, i32>,
    last_move: HashMap<isize, Instant>,
}

impl Herd {
    /// Puts every window of these processes on the monitor. A window created hidden is placed once before it shows, which
    /// is the only way not to see it elsewhere first. Asynchronous: a busy or hung program must not stop the driver's ping.
    fn gather(&mut self, processes: &[u32], bounds: &Bounds) {
        // The keeper runs for days: closed windows leave the lists.
        self.slots.retain(|&handle, _| unsafe { IsWindow(handle as HWND) } != 0);
        self.last_move.retain(|&handle, _| unsafe { IsWindow(handle as HWND) } != 0);
        let mut handles: Vec<isize> = Vec::new();
        unsafe { EnumWindows(Some(crate::collect_handle), &mut handles as *mut Vec<isize> as LPARAM) };
        for handle in handles {
            let hwnd = handle as HWND;
            let mut owner = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut owner) };
            if !processes.contains(&owner) {
                continue;
            }
            let mut rect: RECT = unsafe { std::mem::zeroed() };
            if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 || rect.right - rect.left < SMALLEST_WINDOW || rect.bottom - rect.top < SMALLEST_WINDOW {
                continue;
            }
            if bounds.contains(rect.left, rect.top) {
                continue;
            }
            let visible = unsafe { IsWindowVisible(hwnd) } != 0;
            if !visible && self.slots.contains_key(&handle) {
                continue;
            }
            if self.last_move.get(&handle).is_some_and(|at| at.elapsed() < MOVE_RETRY) {
                continue;
            }
            let next = self.slots.len() as i32;
            let slot = *self.slots.entry(handle).or_insert(next % STAGGER_SLOTS);
            self.last_move.insert(handle, Instant::now());
            unsafe {
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    bounds.left + slot * STAGGER,
                    bounds.top + slot * STAGGER,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                );
            }
        }
    }
}

/// Pings the driver on a thread of its own until `stop`: the driver unplugs the monitor after about a second without one, and
/// nothing the keeper does for a test's windows may hold that up. A failure is noted once, until it answers again.
fn keep_pinging(stop: Arc<AtomicBool>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let driver = match Driver::open() {
            Ok(driver) => driver,
            Err(error) => {
                note_problem(&format!("the monitor keeper could not open the driver to keep the monitor: {error}"));
                return;
            }
        };
        let mut failing = false;
        while !stop.load(Ordering::Relaxed) {
            match driver.ping() {
                Ok(()) => failing = false,
                Err(error) => {
                    if !failing {
                        note_problem(&format!("the driver stopped answering: {error}"));
                    }
                    failing = true;
                }
            }
            std::thread::sleep(Duration::from_millis(LOOP_MS));
        }
    })
}

/// `quietdesk monitor-keeper`: runs until the job has stayed empty with the mode off. Its problems go to `quietdesk status`: it has no console.
pub fn keep() -> i32 {
    // There is one monitor for the whole Windows session, so the keeper keeps its state where the installed programs do, even when a
    // test with a folder of its own started it. Single-threaded here: nothing else runs yet.
    unsafe { std::env::remove_var("QUIETDESK_DIR") };
    let (mutex_name, event_name) = (wide(KEEPER_MUTEX), wide(READY_EVENT));
    unsafe {
        let mutex = CreateMutexW(null(), 1, mutex_name.as_ptr());
        if mutex.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            return 0;
        }
        let ready = CreateEventW(null(), TRUE, 0, event_name.as_ptr());
        ResetEvent(ready);
        // A killed keeper leaves this behind, and with our mutex it would read as a monitor that is there.
        let _ = std::fs::remove_file(bounds_file());
        let driver = match Driver::open() {
            Ok(driver) => driver,
            Err(error) => {
                note_problem(&format!("the monitor keeper could not open the driver: {error}"));
                return 1;
            }
        };
        // A monitor left by a killed keeper goes by itself, but only while nobody pings the driver: adding ours now would keep it for
        // as long as we run, beside the main screen, where the user's mouse can wander onto it.
        let waiting = Instant::now();
        while virtual_display_count() > 0 {
            if waiting.elapsed() >= LEFTOVER_WAIT {
                note_problem("a virtual monitor left by an earlier keeper did not go away, so no new one was plugged in");
                return 1;
            }
            std::thread::sleep(Duration::from_millis(LOOP_MS));
        }
        // The user's own choice of screens, such as "PC screen only", before Windows applies anything for the new set.
        // Without it the monitor still comes, as Windows places it; only keeping the user's choice is lost, and that is noted.
        let before = match Layout::now() {
            Ok(layout) => Some(layout),
            Err(error) => {
                note_problem(&format!("could not read which screens Windows shows: {error}"));
                None
            }
        };
        let index = match driver.add() {
            Ok(index) => index,
            Err(error) => {
                note_problem(&format!("the driver would not add a monitor: {error}"));
                return 1;
            }
        };
        let stop_pinging = Arc::new(AtomicBool::new(false));
        let pinger = keep_pinging(stop_pinging.clone());
        let started = Instant::now();
        let (mut bounds, mut monitor_path) = loop {
            let found = active_displays().into_iter().find(|d| d.is_virtual);
            if let Some(display) = found
                && let Some(settled) = settle(&display, before.as_ref())
            {
                break settled;
            }
            if started.elapsed() > DISPLAY_WAIT {
                note_problem("the virtual monitor never appeared");
                stop_pinging.store(true, Ordering::Relaxed);
                let _ = pinger.join();
                let _ = driver.remove(index);
                return 1;
            }
            std::thread::sleep(Duration::from_millis(LOOP_MS));
        };
        record_bounds(&bounds);
        SetEvent(ready);

        let mut commands = CommandJobs::new(command_jobs_folder());
        let mut jobs_answered = true;
        let mut herd = Herd::default();
        let mut focus = FocusGuard::default();
        let mut family = Family::default();
        let mut empty_since = Instant::now();
        let mut anything_joined = false;
        let mut loops = 0u32;
        let mut looks_without_display = 0u32;
        let mut layout_changes: Vec<Instant> = Vec::new();
        loop {
            if loops.is_multiple_of(FAMILY_EVERY_LOOPS) {
                let in_jobs = commands.processes();
                jobs_answered = in_jobs.is_some();
                family.refresh(in_jobs.as_deref().unwrap_or(&[]));
            }
            loops = loops.wrapping_add(1);
            if loops.is_multiple_of(MODE_EVERY_LOOPS) {
                match active_displays().into_iter().find(|display| display.is_virtual) {
                    Some(display) => {
                        looks_without_display = 0;
                        layout_changes.retain(|at| at.elapsed() < LAYOUT_CHANGE_WINDOW);
                        // A change of the user's screens can move it: windows go where it is now. It can also put it beside one
                        // of them, where the mouse crosses onto it: then it goes back to a corner.
                        if let Some(now) = bounds_of(&display)
                            && now != bounds
                        {
                            let theirs: Vec<Bounds> = active_displays().iter().filter(|d| !d.is_virtual).filter_map(bounds_of).collect();
                            if !apart(&now, &theirs) && layout_changes.len() < LAYOUT_CHANGES {
                                layout_changes.push(Instant::now());
                                if let Err(error) = move_to_a_corner(&display) {
                                    note_problem(&format!("a change of your screens put the virtual monitor beside one, and it could not go back to a corner: {error}"));
                                }
                            }
                            bounds = bounds_of(&display).unwrap_or(now);
                            record_bounds(&bounds);
                        }
                    }
                    None => looks_without_display += 1,
                }
                layout_changes.retain(|at| at.elapsed() < LAYOUT_CHANGE_WINDOW);
                // Switched off, but still plugged in: shown again, which Windows does quietly. Plugging it in anew would play the
                // connect sound and freeze the user's screens at a moment no run may need it.
                if looks_without_display > 0
                    && layout_changes.len() < LAYOUT_CHANGES
                    && let Some(path) = monitor_path
                {
                    layout_changes.push(Instant::now());
                    // What the user's screens are without it now, read before it is shown again.
                    let before = Layout::now();
                    match before.and_then(|before| show_again(&path).map(|()| before)) {
                        Ok(before) => {
                            if let Some(display) = active_displays().into_iter().find(|d| d.is_virtual)
                                && let Some((settled, path)) = settle(&display, Some(&before))
                            {
                                if settled != bounds {
                                    bounds = settled;
                                    record_bounds(&bounds);
                                }
                                monitor_path = path.or(monitor_path);
                                looks_without_display = 0;
                            }
                        }
                        Err(error) => note_problem(&format!("could not show the virtual monitor again after Windows switched it off: {error}")),
                    }
                }
                if looks_without_display >= LOOKS_BEFORE_GONE {
                    note_problem("the virtual monitor went away by itself; the next run that needs it, or quietdesk on, plugs it in again");
                    break;
                }
            }
            let members = family.members();
            focus.check(&members);
            if !members.is_empty() {
                anything_joined = true;
                empty_since = Instant::now();
                herd.gather(&members, &bounds);
            } else if !jobs_answered {
                empty_since = Instant::now();
            } else if empty_since.elapsed() >= (if anything_joined { IDLE_BEFORE_UNPLUG } else { FIRST_COMMAND_WAIT })
                && loops.is_multiple_of(MODE_EVERY_LOOPS)
                && mode_is_certainly_off()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(LOOP_MS));
        }
        ResetEvent(ready);
        let _ = std::fs::remove_file(bounds_file());
        stop_pinging.store(true, Ordering::Relaxed);
        let _ = pinger.join();
        if let Err(error) = driver.remove(index) {
            note_problem(&format!("the driver would not remove the monitor, it unplugs by itself: {error}"));
        }
        CloseHandle(ready);
        CloseHandle(mutex);
    }
    0
}

fn start_keeper() -> io::Result<()> {
    // quietdesk.exe whoever asks: the tray runs as quietdesk-tray.exe beside it, and a second tray would just exit with no word.
    crate::start_apart(&std::env::current_exe()?.with_file_name("quietdesk.exe"), KEEPER_WORD)
}

/// For the switch and the tray: plugs the monitor in and leaves it, so that no run has to while the mode is on. A monitor that
/// cannot be had is no problem here: `quietdesk monitor` says so when a command needs it. The tests' own state folder never plugs one.
pub fn stay_plugged_in_while_the_mode_is_on() {
    if std::env::var_os("QUIETDESK_DIR").is_some() || current_bounds().is_some() || !driver_installed() {
        return;
    }
    if let Err(error) = start_keeper() {
        note_problem(&format!("could not plug in the virtual monitor: {error}"));
    }
}

/// A virtual monitor this process can run commands on.
pub struct Monitor {
    bounds: Bounds,
}

impl Monitor {
    /// Joins the running monitor, or starts one. An error means no monitor: the driver is missing, or it failed.
    pub fn join() -> io::Result<Self> {
        Driver::open()?;
        let event_name = wide(READY_EVENT);
        unsafe {
            let ready = CreateEventW(null(), TRUE, 0, event_name.as_ptr());
            if ready.is_null() {
                return Err(last_error());
            }
            if current_bounds().is_none() {
                start_keeper()?;
            }
            let waited = WaitForSingleObject(ready, READY_WAIT_MS);
            CloseHandle(ready);
            if waited != WAIT_OBJECT_0 {
                return Err(io::Error::other("the virtual monitor did not come up"));
            }
            let bounds = current_bounds().ok_or_else(|| io::Error::other("the virtual monitor went away"))?;
            Ok(Self { bounds })
        }
    }

    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// Runs a command line on the real desktop in a job of its own and waits for it, with this process's console and handles.
    /// It starts suspended, so nothing it starts can escape before it is in the job and the keeper follows that job, however soon
    /// it ends. It asks its first window to open on the monitor without taking the focus; programs that place their own windows
    /// are moved by the keeper.
    pub fn run_and_wait(&self, command_line: &[u16]) -> io::Result<u32> {
        let mut line: Vec<u16> = command_line.iter().copied().take_while(|&c| c != 0).chain(std::iter::once(0)).collect();
        let mut desktop = wide(format!(r"WinSta0\{REAL_DESKTOP}"));
        let mut startup: STARTUPINFOW = unsafe { std::mem::zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        startup.lpDesktop = desktop.as_mut_ptr();
        startup.dwFlags = STARTF_USEPOSITION | STARTF_USESHOWWINDOW;
        startup.dwX = self.bounds.left as u32;
        startup.dwY = self.bounds.top as u32;
        startup.wShowWindow = SW_SHOWNOACTIVATE as u16;
        let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        crate::no_system_error_dialogs();
        crate::leave_ctrl_events_to_the_child_while_waiting();
        let started = unsafe {
            CreateProcessW(null(), line.as_mut_ptr(), null(), null(), TRUE, CREATE_SUSPENDED, null(), null(), &startup, &mut process)
        };
        if started == 0 {
            return Err(last_error());
        }
        // A command nobody follows would leave its windows on the user's screen, unmuted and free to take the focus, so it does not
        // start. A keeper that stopped answering, or ended just as this command came, gets one more chance: a new one if need be.
        let followed = own_job(process.hProcess, process.dwProcessId).filter(|(job, name)| {
            ask_the_keeper_to_follow(&command_jobs_folder(), name) || (Monitor::join().is_ok() && ask_the_keeper_to_follow(&command_jobs_folder(), name)) || {
                unsafe { CloseHandle(*job) };
                false
            }
        });
        let Some((job, _)) = followed else {
            unsafe {
                TerminateProcess(process.hProcess, 1);
                CloseHandle(process.hThread);
            }
            let _ = crate::wait_and_close(process.hProcess);
            return Err(io::Error::other("no virtual monitor keeper could follow the command, so it did not start (see quietdesk status)"));
        };
        unsafe {
            ResumeThread(process.hThread);
            CloseHandle(process.hThread);
        }
        let result = crate::wait_and_close(process.hProcess);
        unsafe { CloseHandle(job) };
        result
    }
}

/// A new job for a command that has not started yet, and the name it goes by; None, with a note, when Windows refuses. Named after
/// the process and its start, so a reused id never meets an older command's job that still runs. The job must not allow breakaway:
/// PowerShell's Start-Process goes through ShellExecute, which leaves any job that allows it, and the game's window would then stay
/// on the user's screen. A new job does not allow it.
fn own_job(process: HANDLE, pid: u32) -> Option<(HANDLE, String)> {
    let file_name = format!("{pid}-{}", started_at(pid).unwrap_or_default());
    let name = command_job_name(OsStr::new(&file_name));
    unsafe {
        let job = CreateJobObjectW(null(), name.as_ptr());
        if job.is_null() || GetLastError() == ERROR_ALREADY_EXISTS {
            note_problem(&format!("a command got no job of its own: {}", last_error()));
            if !job.is_null() {
                CloseHandle(job);
            }
            return None;
        }
        if AssignProcessToJobObject(job, process) == 0 {
            note_problem(&format!("a command could not join its own job: {}", last_error()));
            CloseHandle(job);
            return None;
        }
        Some((job, file_name))
    }
}

/// Tells the keeper about a command's job and waits for its answer, so the job is held before the command can end. False, with a
/// note, when no answer comes. Asked a second time, the file is left as it is: the keeper may have answered meanwhile, and does not
/// answer twice for a job it holds.
fn ask_the_keeper_to_follow(folder: &Path, name: &str) -> bool {
    let file = folder.join(name);
    let created = std::fs::create_dir_all(folder).and_then(|()| std::fs::OpenOptions::new().write(true).create_new(true).open(&file));
    if let Err(error) = created
        && error.kind() != io::ErrorKind::AlreadyExists
    {
        note_problem(&format!("could not tell the keeper about a command: {error}"));
        return false;
    }
    let asked = Instant::now();
    loop {
        match std::fs::read(&file) {
            Ok(answer) if answer == KEEPER_FOLLOWS => return true,
            // Not yet, or half written.
            Ok(_) => {}
            // Only the keeper removes it, and only once the job is empty: never while this command waits in it.
            Err(error) => {
                note_problem(&format!("the keeper's answer about a command went missing: {error}"));
                return false;
            }
        }
        if asked.elapsed() >= KEEPER_ANSWER_WAIT {
            note_problem("the keeper did not answer in time");
            return false;
        }
        std::thread::sleep(KEEPER_ANSWER_LOOK);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Bounds, CommandJobs, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE, DISPLAYCONFIG_PATH_INFO,
        DISPLAYCONFIG_PATH_MODE_IDX_INVALID, Family, KEEPER_FOLLOWS, Layout, POINTL, apart, ask_the_keeper_to_follow, free_corner, own_job,
        process_table,
    };
    use windows_sys::Win32::Devices::Display::DISPLAYCONFIG_SOURCE_MODE;
    use crate::wide;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::ptr::null;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW};
    use windows_sys::Win32::System::Threading::{
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
    };

    /// From a job nothing may leave, as with Claude Code's Bash tool, a command still gets its own job. The keeper holds that job while
    /// anything runs in it, even after the command's own `quietdesk monitor` has closed it, then forgets it; a job nobody holds any more
    /// leaves no file behind. Runs the keeper's side without a keeper or a driver, and opens no window: the command never starts.
    #[test]
    fn a_commands_job_is_followed_from_inside_any_job_until_it_is_empty() {
        let folder = std::env::temp_dir().join(format!("quietdesk-command-jobs-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let stale = folder.join("4294967295");
        std::fs::write(&stale, b"").unwrap();

        let mut line = wide("cmd.exe /c exit 0");
        let startup = STARTUPINFOW { cb: size_of::<STARTUPINFOW>() as u32, ..unsafe { std::mem::zeroed() } };
        let mut process: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let flags = CREATE_SUSPENDED | CREATE_NO_WINDOW;
        let started = unsafe { CreateProcessW(null(), line.as_mut_ptr(), null(), null(), 0, flags, null(), null(), &startup, &mut process) };
        assert_ne!(started, 0, "the test's command did not start");
        let outer = unsafe { CreateJobObjectW(null(), null()) };
        assert_eq!(unsafe { AssignProcessToJobObject(outer, process.hProcess) }, 1, "the outer job took it");

        let (job, name) = own_job(process.hProcess, process.dwProcessId).expect("a command inside another job must still get its own");

        let pid = process.dwProcessId;
        let asking_in = folder.clone();
        let file = folder.join(&name);
        let asking = std::thread::spawn(move || ask_the_keeper_to_follow(&asking_in, &name));
        let mut jobs = CommandJobs::new(folder.clone());
        let started_asking = Instant::now();
        while !file.exists() && started_asking.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(jobs.processes(), Some(vec![pid]), "the keeper sees the command");
        assert_eq!(std::fs::read(&file).unwrap(), KEEPER_FOLLOWS);
        assert!(asking.join().unwrap(), "the command heard the answer");
        assert!(!stale.exists(), "a file whose job nobody holds is removed");

        unsafe { CloseHandle(job) };
        assert_eq!(jobs.processes(), Some(vec![pid]), "held after quietdesk monitor let go, while something runs in it");

        unsafe {
            TerminateProcess(process.hProcess, 0);
            WaitForSingleObject(process.hProcess, 5000);
        }
        let mut last = jobs.processes();
        let ending = Instant::now();
        while last != Some(Vec::new()) && ending.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
            last = jobs.processes();
        }
        assert_eq!(last, Some(Vec::new()), "an empty job is forgotten");
        assert!(!file.exists(), "and its file with it");
        assert!(jobs.open.is_empty());

        unsafe {
            CloseHandle(process.hThread);
            CloseHandle(process.hProcess);
            CloseHandle(outer);
        }
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A command asks again after it started a new keeper: the answer the first one wrote counts, as that keeper holds the job and
    /// will not write it again. An answer that went missing counts as none, quickly. No keeper, no driver, no window.
    #[test]
    fn asked_again_the_earlier_answer_counts_and_a_missing_one_does_not() {
        let folder = std::env::temp_dir().join(format!("quietdesk-asked-again-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("answered"), KEEPER_FOLLOWS).unwrap();
        let asked = Instant::now();
        assert!(ask_the_keeper_to_follow(&folder, "answered"), "the keeper's earlier answer was not heard");
        assert!(asked.elapsed() < Duration::from_secs(1), "it waited for an answer it already had");
        assert_eq!(std::fs::read(folder.join("answered")).unwrap(), KEEPER_FOLLOWS, "asking again wiped the answer");

        let removing = folder.join("removed");
        let remover = std::thread::spawn({
            let removing = removing.clone();
            move || {
                let waiting = Instant::now();
                while !removing.exists() && waiting.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                std::fs::remove_file(&removing).unwrap();
            }
        });
        let asked = Instant::now();
        assert!(!ask_the_keeper_to_follow(&folder, "removed"), "a missing answer counted as an answer");
        assert!(asked.elapsed() < Duration::from_secs(1), "a missing answer was waited for until the time ran out");
        remover.join().unwrap();
        let noted = std::fs::read_to_string(crate::state_dir().join("problems.log")).unwrap_or_default();
        assert!(noted.contains("went missing"), "the problem was not noted in the tests' own state folder");
        let _ = std::fs::remove_dir_all(crate::state_dir());
        let _ = std::fs::remove_dir_all(&folder);
    }

    fn screen_at(x: i32, y: i32, width: u32, height: u32) -> DISPLAYCONFIG_MODE_INFO {
        let mut mode: DISPLAYCONFIG_MODE_INFO = unsafe { std::mem::zeroed() };
        mode.infoType = DISPLAYCONFIG_MODE_INFO_TYPE_SOURCE;
        mode.Anonymous.sourceMode = DISPLAYCONFIG_SOURCE_MODE { width, height, pixelFormat: 0, position: POINTL { x, y } };
        mode
    }

    fn path_on(source_mode: u32) -> DISPLAYCONFIG_PATH_INFO {
        let mut path: DISPLAYCONFIG_PATH_INFO = unsafe { std::mem::zeroed() };
        path.sourceInfo.Anonymous.modeInfoIdx = source_mode;
        path.targetInfo.Anonymous.modeInfoIdx = DISPLAYCONFIG_PATH_MODE_IDX_INVALID;
        path
    }

    /// Windows put the monitor beside one of the user's screens: it goes to a corner, at its own size, and their screens stay where
    /// they were. A layout where it would be the main screen, or show a copy of one of theirs, is refused, never kept.
    #[test]
    fn the_monitor_goes_to_a_corner_and_a_layout_that_must_not_be_kept_is_refused() {
        // Theirs: the main screen and one to its right. The monitor right below that one, sharing an edge with both.
        let (main, right) = (screen_at(0, 0, 2560, 1440), screen_at(2560, 0, 1920, 1080));
        let mut layout = Layout { paths: vec![path_on(0), path_on(1), path_on(2)], modes: vec![main, right, screen_at(2560, 1080, 2560, 1440)] };
        let theirs = [Bounds { left: 0, top: 0, right: 2560, bottom: 1440 }, Bounds { left: 2560, top: 0, right: 4480, bottom: 1080 }];
        assert!(!apart(&layout.screens()[2].1, &theirs), "the test must start with the monitor where the mouse reaches it");
        layout.place_at_a_corner(2).unwrap();
        let screens = layout.screens();
        assert_eq!([screens[0].1, screens[1].1], theirs, "the user's own screens moved");
        let monitor = screens[2].1;
        assert!(apart(&monitor, &theirs), "the monitor is still where the mouse reaches it: {monitor:?}");
        assert_eq!((monitor.right - monitor.left, monitor.bottom - monitor.top), (2560, 1440), "the monitor changed size");
        layout.place_at_a_corner(2).unwrap();
        assert_eq!(layout.screens()[2].1, monitor, "a monitor at a corner already was moved");

        // An earlier keeper's monitor, still listed, goes; two screens showing one picture still share it.
        let (mut first, mut copy, mut stale) = (path_on(0), path_on(0), path_on(1));
        first.targetInfo.Anonymous.modeInfoIdx = 2;
        copy.targetInfo.id = 1;
        stale.targetInfo.adapterId.LowPart = 7;
        let listed = Layout { paths: vec![first, stale, copy], modes: vec![main, screen_at(2560, 0, 1920, 1080), screen_at(5, 5, 5, 5)] };
        let kept = listed.keeping(|path| path.targetInfo.adapterId.LowPart != 7);
        assert_eq!(kept.paths.len(), 2);
        assert_eq!(kept.modes.len(), 2, "a mode of the path that went stayed, or a shared one was copied");
        let source = |path: &DISPLAYCONFIG_PATH_INFO| unsafe { path.sourceInfo.Anonymous.modeInfoIdx };
        assert_eq!(source(&kept.paths[0]), source(&kept.paths[1]), "the copy no longer shares its picture");
        assert_eq!(kept.screens()[0].1, theirs[0]);

        // None of theirs is the main screen at 0,0: the monitor is.
        let mut monitor_is_main = Layout { paths: vec![path_on(0), path_on(1)], modes: vec![screen_at(-2560, 0, 2560, 1440), screen_at(0, 0, 2560, 1440)] };
        assert!(monitor_is_main.place_at_a_corner(1).is_err(), "a layout with the monitor as the main screen was accepted");
        // The monitor shows a copy of their screen: moving it would move theirs.
        let mut copy = Layout { paths: vec![path_on(0), path_on(0)], modes: vec![main] };
        assert!(copy.place_at_a_corner(1).is_err(), "a monitor that copies the user's screen was accepted");
        assert_eq!(copy.screens()[0].1, theirs[0], "the user's screen moved");
    }

    /// How long the test's children live, in pings a second apart: long enough to be seen, gone soon after a test, as killing
    /// `cmd` does not reach its own child.
    const CHILD_PINGS: u32 = 5;

    /// The Store's PowerShell 7 lets its children out of any job, so what keeps a game on the monitor is following the
    /// family. Opens no window. Fails if children stop being followed, or if a stranger is taken in.
    #[test]
    fn a_family_takes_in_children_and_grandchildren_but_no_strangers() {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let spawn = |marker: &str| {
            Command::new("cmd.exe")
                .args(["/c", &format!("ping -n {CHILD_PINGS} 127.0.0.{marker} >nul")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .spawn()
                .unwrap()
        };
        let mut member = spawn("1");
        let mut stranger = spawn("2");
        let mut family = Family::default();
        let started = Instant::now();
        let mut members = Vec::new();
        while members.len() < 2 && started.elapsed() < Duration::from_secs(10) {
            family.refresh(&[member.id()]);
            members = family.members();
            std::thread::sleep(Duration::from_millis(50));
        }
        for child in [&mut member, &mut stranger] {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(members.contains(&member.id()), "the seed itself");
        assert!(members.len() >= 2, "its child, ping, was not followed: {members:?}");
        assert!(!members.contains(&stranger.id()), "a process outside the family was taken in");
        assert!(!members.contains(&std::process::id()), "the family's own parent was taken in");
    }

    /// An id that is reused must not stay in the family, and the new holder's children must not join it: the new process could be
    /// the user's browser or game. Here a member's recorded start no longer matches the process holding its id, which has a child.
    #[test]
    fn a_reused_id_and_its_holders_children_stay_out_of_the_family() {
        let mut family = Family::default();
        let other = Command::new("cmd.exe").args(["/c", &format!("ping -n {CHILD_PINGS} 127.0.0.3 >nul")]).stdout(Stdio::null()).spawn();
        let mut other = other.unwrap();
        let has_child = |pid: u32| process_table().is_some_and(|table| table.iter().any(|&(_, parent)| parent == pid));
        let waiting = Instant::now();
        while !has_child(other.id()) && waiting.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(has_child(other.id()), "the holder's child, ping, started");
        family.live.insert(other.id(), 1);
        family.refresh(&[]);
        let members = family.members();
        let _ = other.kill();
        let _ = other.wait();
        assert!(members.is_empty(), "neither the new holder nor its child belongs to the family: {members:?}");
    }

    #[test]
    fn bounds_tell_inside_from_outside_and_a_shared_edge_from_a_corner() {
        let primary = Bounds { left: 0, top: 0, right: 2560, bottom: 1440 };
        assert!(primary.contains(0, 0) && primary.contains(2559, 1439));
        assert!(!primary.contains(2560, 0) && !primary.contains(-1, 5));
        let corner = Bounds { left: 2560, top: 1440, right: 5120, bottom: 2880 };
        assert!(!primary.shares_more_than_a_corner(&corner) && !corner.shares_more_than_a_corner(&primary));
        let beside = Bounds { left: 2560, top: 0, right: 4480, bottom: 1080 };
        assert!(primary.shares_more_than_a_corner(&beside), "a shared edge");
        assert!(primary.shares_more_than_a_corner(&Bounds { left: 2559, top: 1439, right: 2600, bottom: 1500 }), "an overlap");
    }

    /// The monitor's place: a corner of the main screen that no screen shares an edge with, so the mouse cannot reach it.
    #[test]
    fn the_monitor_goes_where_no_screen_shares_an_edge_with_it() {
        let main = Bounds { left: 0, top: 0, right: 2560, bottom: 1440 };
        let (width, height) = (2560, 1440);
        // A second screen to the left, as on the author's PC: the bottom-right corner is free.
        let left = Bounds { left: -1920, top: 52, right: 0, bottom: 1132 };
        assert_eq!(free_corner(main, &[main, left], width, height), Some((2560, 1440)));
        // One as tall to the right: there the monitor would share that screen's bottom edge, so bottom-left instead.
        let right = Bounds { left: 2560, top: 0, right: 5120, bottom: 1440 };
        assert_eq!(free_corner(main, &[main, right], width, height), Some((-2560, 1440)));
        // Screens all round the main one's corners, sharing every edge a corner spot would have: none is free there, so another
        // screen's corner is used.
        let below = Bounds { left: -2560, top: 1440, right: 5120, bottom: 2880 };
        let above = Bounds { left: -2560, top: -1440, right: 5120, bottom: 0 };
        let spot = free_corner(main, &[main, below, above], width, height).expect("some corner is free");
        let spot = Bounds { left: spot.0, top: spot.1, right: spot.0 + width, bottom: spot.1 + height };
        assert!(![main, below, above].iter().any(|screen| spot.shares_more_than_a_corner(screen)), "{spot:?}");
    }
}
