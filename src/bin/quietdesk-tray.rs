//! The tray icon: shows whether QuietDesk is on, flips it with a click, and opens a live preview of what runs hidden,
//! where a stuck window can also be closed. The mode does not depend on this program: closing it changes nothing.

#![windows_subsystem = "windows"]

use std::cell::RefCell;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

use quietdesk::{Mode, Picture, Window, process_name, read_mode, set_mode, wide};
use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, COLOR_BTNFACE, COLOR_WINDOW, CreateBitmap, CreateCompatibleDC,
    CreateDIBSection, DEFAULT_GUI_FONT, DIB_RGB_COLORS, DT_CENTER, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW,
    EndPaint, ExcludeClipRect, FillRect, GetStockObject, GetSysColorBrush, HALFTONE, InvalidateRect, PAINTSTRUCT, SRCCOPY,
    SelectClipRgn, SetBkMode, SetStretchBltMode, StretchDIBits, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BN_CLICKED, CW_USEDEFAULT, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, HICON, ICONINFO,
    IDC_ARROW, LB_ADDSTRING, LB_ERR, LB_GETCURSEL, LB_RESETCONTENT, LB_SETCURSEL, LBN_SELCHANGE, LBS_NOTIFY,
    LoadCursorW, MB_ICONWARNING, MB_OK, MF_CHECKED, MF_DISABLED, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, MoveWindow, PostMessageW,
    PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SendMessageW, SetForegroundWindow, SetTimer, ShowWindow,
    SW_SHOWNORMAL, TPM_NONOTIFY, TPM_RETURNCMD, TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE, WM_COMMAND, WM_DESTROY,
    WM_ERASEBKGND, WM_LBUTTONUP, WM_NULL, WM_PAINT, WM_RBUTTONUP, WM_SETFONT, WM_SIZE, WM_TIMER, WNDCLASSW, WS_BORDER,
    WS_CHILD, WS_OVERLAPPED, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};

const TRAY_CLASS: &str = quietdesk::TRAY_WINDOW_CLASS;
const PREVIEW_CLASS: &str = "QuietDeskPreviewWindow";
const TRAY_ICON_ID: u32 = 1;
const TRAY_MESSAGE: u32 = WM_APP + 1;
const PICTURE_READY: u32 = WM_APP + 2;
const TIMER_MODE: usize = 1;
const TIMER_PREVIEW: usize = 2;
/// A flip from a session shows on the icon within this time.
const MODE_POLL_MS: u32 = 1000;
const PREVIEW_MS: u32 = 700;
const MENU_FLIP: u32 = 100;
const MENU_PREVIEW: u32 = 101;
const MENU_QUIT: u32 = 102;
const MENU_SOUND: u32 = 103;
const CONTROL_LIST: usize = 10;
const CONTROL_CLOSE: usize = 11;
const LIST_WIDTH: i32 = 320;
const BUTTON_HEIGHT: i32 = 40;
const ICON_SIZE: i32 = 32;

struct Preview {
    window: HWND,
    list: HWND,
    button: HWND,
    windows: Vec<Window>,
    selected: Option<isize>,
    picture: Option<Picture>,
    capturing: bool,
}

struct App {
    tray: HWND,
    icon_on: HICON,
    icon_off: HICON,
    shown: Mode,
    taskbar_created: u32,
    preview: Option<Preview>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `action` on the app state. Window messages can arrive while it is in use (a control being created or resized);
/// those are left to the default handling rather than nested.
fn with_app<T>(action: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|app| app.try_borrow_mut().ok().and_then(|mut app| app.as_mut().map(action)))
}

/// The usual sign: a red disc with a white bar when on, a grey ring when off.
unsafe fn make_icon(mode: Mode) -> HICON {
    unsafe {
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = ICON_SIZE;
        info.bmiHeader.biHeight = -ICON_SIZE;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let dc = CreateCompatibleDC(null_mut());
        let mut bits = null_mut();
        let color = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u32, (ICON_SIZE * ICON_SIZE) as usize);
        let center = (ICON_SIZE as f32 - 1.0) / 2.0;
        for y in 0..ICON_SIZE {
            for x in 0..ICON_SIZE {
                let (dx, dy) = (x as f32 - center, y as f32 - center);
                let distance = (dx * dx + dy * dy).sqrt();
                pixels[(y * ICON_SIZE + x) as usize] = match mode {
                    Mode::On if dx.abs() < 9.5 && dy.abs() < 3.5 => 0xFFFF_FFFF,
                    Mode::On if distance < 15.0 => 0xFFD9_3025,
                    Mode::Off if (10.5..15.0).contains(&distance) => 0xFF9A_A0A6,
                    _ => 0,
                };
            }
        }
        let mask = CreateBitmap(ICON_SIZE, ICON_SIZE, 1, 1, null());
        let icon_info = ICONINFO { fIcon: 1, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&icon_info);
        DeleteObject(mask);
        DeleteObject(color);
        DeleteDC(dc);
        icon
    }
}

fn tip(mode: Mode) -> &'static str {
    match mode {
        Mode::On => "QuietDesk is ON: Claude's test windows stay off your screen. Click to turn off.",
        Mode::Off => "QuietDesk is OFF: windows open normally. Click to turn on.",
    }
}

unsafe fn tray_icon(action: u32, app: &App) {
    unsafe {
        let mut data: NOTIFYICONDATAW = std::mem::zeroed();
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = app.tray;
        data.uID = TRAY_ICON_ID;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TRAY_MESSAGE;
        data.hIcon = if app.shown == Mode::On { app.icon_on } else { app.icon_off };
        let text: Vec<u16> = tip(app.shown).encode_utf16().take(data.szTip.len() - 1).collect();
        data.szTip[..text.len()].copy_from_slice(&text);
        Shell_NotifyIconW(action, &data);
    }
}

fn flip() {
    let wanted = if read_mode() == Mode::On { Mode::Off } else { Mode::On };
    if let Err(error) = set_mode(wanted) {
        let (text, title) = (wide(format!("The switch reported a problem: {error}")), wide("QuietDesk"));
        unsafe { MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING) };
    }
    refresh_icon();
}

fn refresh_icon() {
    let mode = read_mode();
    with_app(|app| {
        if app.shown != mode {
            app.shown = mode;
            unsafe { tray_icon(NIM_MODIFY, app) };
        }
    });
}

unsafe fn show_menu(tray: HWND) {
    unsafe {
        let menu = CreatePopupMenu();
        let mode = read_mode();
        let status = wide(if mode == Mode::On { "QuietDesk is ON" } else { "QuietDesk is OFF" });
        let flip_text = wide(if mode == Mode::On { "Turn off" } else { "Turn on" });
        let preview_text = wide("Show what runs hidden...");
        let sound_text = wide("Mute test sounds");
        let sound_flags = if quietdesk::sound::muting_enabled() { MF_STRING | MF_CHECKED } else { MF_STRING };
        let quit_text = wide("Close this icon (the mode keeps working, but test sounds are no longer muted)");
        AppendMenuW(menu, MF_STRING | MF_DISABLED, 0, status.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_FLIP as usize, flip_text.as_ptr());
        AppendMenuW(menu, sound_flags, MENU_SOUND as usize, sound_text.as_ptr());
        AppendMenuW(menu, MF_STRING, MENU_PREVIEW as usize, preview_text.as_ptr());
        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        AppendMenuW(menu, MF_STRING, MENU_QUIT as usize, quit_text.as_ptr());
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        // Windows closes a tray menu on a click elsewhere only if its owner is in front; the user's window is invisible.
        SetForegroundWindow(tray);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, cursor.x, cursor.y, 0, tray, null()) as u32;
        PostMessageW(tray, WM_NULL, 0, 0);
        DestroyMenu(menu);
        match chosen {
            MENU_FLIP => flip(),
            MENU_SOUND => {
                if let Err(error) = quietdesk::sound::set_muting(!quietdesk::sound::muting_enabled()) {
                    let (text, title) = (wide(format!("Could not save that: {error}")), wide("QuietDesk"));
                    MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING);
                }
            }
            MENU_PREVIEW => open_preview(),
            MENU_QUIT => {
                DestroyWindow(tray);
            }
            _ => {}
        }
    }
}

unsafe extern "system" fn tray_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            TRAY_MESSAGE => {
                match lparam as u32 {
                    WM_LBUTTONUP => flip(),
                    WM_RBUTTONUP => show_menu(hwnd),
                    _ => {}
                }
                0
            }
            WM_TIMER => {
                refresh_icon();
                0
            }
            WM_DESTROY => {
                with_app(|app| tray_icon(NIM_DELETE, app));
                PostQuitMessage(0);
                0
            }
            _ => {
                // Explorer restarted: the icon has to be added again.
                let restarted = with_app(|app| app.taskbar_created == message).unwrap_or(false);
                if restarted {
                    with_app(|app| tray_icon(NIM_ADD, app));
                    return 0;
                }
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
    }
}

unsafe fn open_preview() {
    unsafe {
        if let Some(Some(existing)) = with_app(|app| app.preview.as_ref().map(|preview| preview.window as isize)) {
            ShowWindow(existing as HWND, SW_SHOWNORMAL);
            SetForegroundWindow(existing as HWND);
            return;
        }
        let instance = GetModuleHandleW(null());
        let (class, title) = (wide(PREVIEW_CLASS), wide("QuietDesk: what runs hidden"));
        let window = CreateWindowExW(
            0, class.as_ptr(), title.as_ptr(), WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT, CW_USEDEFAULT, 1100, 680, null_mut(), null_mut(), instance, null(),
        );
        let (list_class, button_class, button_text, empty) = (wide("LISTBOX"), wide("BUTTON"), wide("Close this window"), wide(""));
        let list = CreateWindowExW(
            0, list_class.as_ptr(), empty.as_ptr(), WS_CHILD | WS_VISIBLE | WS_BORDER | WS_VSCROLL | LBS_NOTIFY as u32,
            0, 0, LIST_WIDTH, 100, window, CONTROL_LIST as _, instance, null(),
        );
        let button = CreateWindowExW(
            0, button_class.as_ptr(), button_text.as_ptr(), WS_CHILD | WS_VISIBLE,
            0, 0, LIST_WIDTH, BUTTON_HEIGHT, window, CONTROL_CLOSE as _, instance, null(),
        );
        let font = GetStockObject(DEFAULT_GUI_FONT);
        SendMessageW(list, WM_SETFONT, font as usize, 1);
        SendMessageW(button, WM_SETFONT, font as usize, 1);
        with_app(|app| {
            app.preview = Some(Preview { window, list, button, windows: Vec::new(), selected: None, picture: None, capturing: false });
        });
        lay_out(window);
        SetTimer(window, TIMER_PREVIEW, PREVIEW_MS, None);
        preview_tick();
    }
}

unsafe fn lay_out(window: HWND) {
    unsafe {
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(window, &mut client);
        with_app(|app| {
            if let Some(preview) = &app.preview {
                MoveWindow(preview.list, 0, 0, LIST_WIDTH, client.bottom - BUTTON_HEIGHT, 1);
                MoveWindow(preview.button, 0, client.bottom - BUTTON_HEIGHT, LIST_WIDTH, BUTTON_HEIGHT, 1);
            }
        });
        InvalidateRect(window, null(), 0);
    }
}

fn list_label(window: &Window) -> String {
    let title = if window.title.is_empty() { window.class.as_str() } else { window.title.as_str() };
    format!("{}: {} ({}x{})", process_name(window.pid), title, window.width, window.height)
}

/// Updates the list and asks for a new picture of the selected window. A capture runs on its own thread, one at a time:
/// asking a hung program for its picture must not freeze this window.
unsafe fn preview_tick() {
    let current = quietdesk::background_windows().unwrap_or_default();
    let request = with_app(|app| {
        let preview = app.preview.as_mut()?;
        let changed = current.len() != preview.windows.len()
            || current.iter().zip(&preview.windows).any(|(a, b)| a.hwnd != b.hwnd || a.title != b.title);
        if changed {
            unsafe {
                SendMessageW(preview.list, LB_RESETCONTENT, 0, 0);
                for window in &current {
                    let label = wide(list_label(window));
                    SendMessageW(preview.list, LB_ADDSTRING, 0, label.as_ptr() as LPARAM);
                }
            }
        }
        preview.windows = current.clone();
        if !preview.windows.iter().any(|w| Some(w.hwnd) == preview.selected) {
            preview.selected = preview.windows.first().map(|w| w.hwnd);
            preview.picture = None;
        }
        if let Some(index) = preview.windows.iter().position(|w| Some(w.hwnd) == preview.selected) {
            unsafe { SendMessageW(preview.list, LB_SETCURSEL, index, 0) };
        }
        unsafe { InvalidateRect(preview.window, null(), 0) };
        if preview.capturing {
            return None;
        }
        let selected = preview.windows.iter().find(|w| Some(w.hwnd) == preview.selected)?.clone();
        preview.capturing = true;
        Some((preview.window as isize, selected))
    });
    if let Some(Some((target, window))) = request {
        std::thread::spawn(move || {
            let picture = quietdesk::capture(&window).ok();
            let message = Box::into_raw(Box::new((window.hwnd, picture)));
            unsafe {
                if PostMessageW(target as HWND, PICTURE_READY, 0, message as LPARAM) == 0 {
                    drop(Box::from_raw(message));
                }
            }
        });
    }
}

unsafe fn paint_preview(window: HWND) {
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(window, &mut paint);
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(window, &mut client);
        let area = RECT { left: LIST_WIDTH, top: 0, right: client.right, bottom: client.bottom };
        let (area_width, area_height) = (area.right - area.left, area.bottom - area.top);
        let background = GetSysColorBrush(COLOR_BTNFACE);
        with_app(|app| {
            let Some(preview) = &app.preview else { return };
            let message = match &preview.picture {
                Some(picture) if area_width > 0 && area_height > 0 && picture.width > 0 && picture.height > 0 => {
                    let scale = (area_width as f32 / picture.width as f32).min(area_height as f32 / picture.height as f32);
                    let (width, height) = ((picture.width as f32 * scale) as i32, (picture.height as f32 * scale) as i32);
                    let (left, top) = (area.left + (area_width - width) / 2, area.top + (area_height - height) / 2);
                    let mut info: BITMAPINFO = std::mem::zeroed();
                    info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
                    info.bmiHeader.biWidth = picture.width as i32;
                    info.bmiHeader.biHeight = -(picture.height as i32);
                    info.bmiHeader.biPlanes = 1;
                    info.bmiHeader.biBitCount = 32;
                    info.bmiHeader.biCompression = BI_RGB;
                    SetStretchBltMode(dc, HALFTONE);
                    StretchDIBits(
                        dc, left, top, width, height, 0, 0, picture.width as i32, picture.height as i32,
                        picture.bgra.as_ptr().cast(), &info, DIB_RGB_COLORS, SRCCOPY,
                    );
                    ExcludeClipRect(dc, left, top, left + width, top + height);
                    FillRect(dc, &area, background);
                    SelectClipRgn(dc, null_mut());
                    picture.is_black().then_some(
                        "This window comes out black from outside: OpenGL windows cannot be captured on the hidden desktop. \
                         It is still running, and the game's own screenshots work. On the virtual monitor (quietdesk monitor) its picture shows here.",
                    )
                }
                _ => {
                    FillRect(dc, &area, background);
                    Some(if preview.windows.is_empty() { "Nothing is running hidden right now." } else { "Waiting for a picture..." })
                }
            };
            if let Some(text) = message {
                let mut text_area = RECT { left: area.left + 24, top: 24, right: area.right - 24, bottom: area.bottom };
                let text = wide(text);
                SetBkMode(dc, TRANSPARENT as i32);
                let old = windows_sys::Win32::Graphics::Gdi::SelectObject(dc, GetStockObject(DEFAULT_GUI_FONT));
                DrawTextW(dc, text.as_ptr(), -1, &mut text_area, DT_CENTER | DT_WORDBREAK);
                windows_sys::Win32::Graphics::Gdi::SelectObject(dc, old);
            }
        });
        EndPaint(window, &paint);
    }
}

unsafe extern "system" fn preview_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_SIZE => {
                lay_out(hwnd);
                0
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                paint_preview(hwnd);
                0
            }
            WM_TIMER => {
                preview_tick();
                0
            }
            PICTURE_READY => {
                let result = Box::from_raw(lparam as *mut (isize, Option<Picture>));
                let (from, picture) = *result;
                with_app(|app| {
                    if let Some(preview) = &mut app.preview {
                        preview.capturing = false;
                        if preview.selected == Some(from) {
                            preview.picture = picture;
                            InvalidateRect(preview.window, null(), 0);
                        }
                    }
                });
                0
            }
            WM_COMMAND => {
                let (control, notification) = (wparam & 0xFFFF, (wparam >> 16) & 0xFFFF);
                with_app(|app| {
                    let Some(preview) = &mut app.preview else { return };
                    if control == CONTROL_LIST && notification == LBN_SELCHANGE as usize {
                        let index = SendMessageW(preview.list, LB_GETCURSEL, 0, 0);
                        if index != LB_ERR as isize
                            && let Some(window) = preview.windows.get(index as usize)
                        {
                            preview.selected = Some(window.hwnd);
                            preview.picture = None;
                        }
                    } else if control == CONTROL_CLOSE
                        && notification == BN_CLICKED as usize
                        && let Some(window) = preview.windows.iter().find(|w| Some(w.hwnd) == preview.selected)
                    {
                        // The same request as the window's X button: the program decides.
                        let _ = quietdesk::post_to_window(window, WM_CLOSE, 0, 0);
                    }
                });
                0
            }
            WM_DESTROY => {
                with_app(|app| app.preview = None);
                0
            }
            _ => DefWindowProcW(hwnd, message, wparam, lparam),
        }
    }
}

fn main() {
    unsafe {
        let mutex_name = wide(quietdesk::TRAY_MUTEX);
        CreateMutexW(null(), 1, mutex_name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let instance = GetModuleHandleW(null());
        let cursor = LoadCursorW(null_mut(), IDC_ARROW);
        let (tray_class, preview_class, taskbar) = (wide(TRAY_CLASS), wide(PREVIEW_CLASS), wide("TaskbarCreated"));
        let mut class: WNDCLASSW = std::mem::zeroed();
        class.lpfnWndProc = Some(tray_proc);
        class.hInstance = instance;
        class.lpszClassName = tray_class.as_ptr();
        RegisterClassW(&class);
        class.lpfnWndProc = Some(preview_proc);
        class.hCursor = cursor;
        class.hbrBackground = GetSysColorBrush(COLOR_WINDOW);
        class.lpszClassName = preview_class.as_ptr();
        RegisterClassW(&class);

        let tray = CreateWindowExW(
            0, tray_class.as_ptr(), tray_class.as_ptr(), WS_OVERLAPPED,
            0, 0, 0, 0, null_mut(), null_mut(), instance, null(),
        );
        let app = App {
            tray,
            icon_on: make_icon(Mode::On),
            icon_off: make_icon(Mode::Off),
            shown: read_mode(),
            taskbar_created: RegisterWindowMessageW(taskbar.as_ptr()),
            preview: None,
        };
        tray_icon(NIM_ADD, &app);
        let (icon_on, icon_off) = (app.icon_on, app.icon_off);
        APP.with(|slot| *slot.borrow_mut() = Some(app));
        SetTimer(tray, TIMER_MODE, MODE_POLL_MS, None);

        // At sign-in with the mode already on: before any game, and no run has to plug it in later.
        if read_mode() == Mode::On {
            quietdesk::monitor::stay_plugged_in_while_the_mode_is_on();
        }
        static STOP_SOUND_GUARD: AtomicBool = AtomicBool::new(false);
        let sound_guard = std::thread::spawn(|| quietdesk::sound::run_guard(&STOP_SOUND_GUARD));

        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        STOP_SOUND_GUARD.store(true, Ordering::Relaxed);
        let _ = sound_guard.join();
        DestroyIcon(icon_on);
        DestroyIcon(icon_off);
    }
}
