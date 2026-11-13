//! The tray icon: shows whether QuietDesk is on, flips it with a click, keeps the list of programs always on the user's screen,
//! and opens a live preview of what runs hidden, where a stuck window can also be closed. The mode does not depend on this
//! program: closing it changes nothing.

#![cfg_attr(not(test), windows_subsystem = "windows")]

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
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, OFN_DONTADDTORECENT, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, BN_CLICKED, CW_USEDEFAULT, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetClientRect, GetCursorPos, GetMessageW, HICON, HMENU, ICONINFO,
    IDC_ARROW, LB_ADDSTRING, LB_ERR, LB_GETCURSEL, LB_RESETCONTENT, LB_SETCURSEL, LBN_SELCHANGE, LBS_NOTIFY,
    LoadCursorW, MB_ICONWARNING, MB_OK, MF_CHECKED, MF_DISABLED, MF_POPUP, MF_SEPARATOR, MF_STRING, MSG, MessageBoxW, MoveWindow, PostMessageW,
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
const MENU_ADD_FILE: u32 = 104;
/// The programs on the list, then the open programs to add, each numbered from here: up to `MENU_PROGRAMS` of each.
const MENU_LISTED: u32 = 1000;
const MENU_OPEN: u32 = 2000;
const MENU_PROGRAMS: usize = 500;
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
        Mode::On => "QuietDesk is ON: test windows stay off your screen. Click to turn off.",
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

fn warn(text: &str) {
    let (text, title) = (wide(text), wide("QuietDesk"));
    unsafe { MessageBoxW(null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING) };
}

fn flip() {
    let wanted = if read_mode() == Mode::On { Mode::Off } else { Mode::On };
    if let Err(error) = set_mode(wanted) {
        warn(&format!("The switch reported a problem: {error}"));
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

/// What a command in the tray's menu does.
#[derive(Debug, PartialEq)]
enum Choice {
    Flip,
    Sound,
    Preview,
    Quit,
    /// Puts a program on the list of those always on the user's screen.
    Keep(String),
    /// Takes one off it.
    Release(String),
    /// Asks for a program file to put on it.
    KeepFile,
}

unsafe fn add_item(menu: HMENU, flags: u32, id: usize, text: &str) {
    let text = wide(text);
    unsafe { AppendMenuW(menu, flags, id, text.as_ptr()) };
}

/// The tray's menu, and what each of its commands does. The list and the open programs come in, so a test can give its own.
unsafe fn build_menu(mode: Mode, muting: bool, listed: &[String], open: &[String]) -> (HMENU, Vec<(u32, Choice)>) {
    unsafe {
        let mut choices = vec![
            (MENU_FLIP, Choice::Flip),
            (MENU_SOUND, Choice::Sound),
            (MENU_PREVIEW, Choice::Preview),
            (MENU_QUIT, Choice::Quit),
            (MENU_ADD_FILE, Choice::KeepFile),
        ];
        let menu = CreatePopupMenu();
        add_item(menu, MF_STRING | MF_DISABLED, 0, if mode == Mode::On { "QuietDesk is ON" } else { "QuietDesk is OFF" });
        add_item(menu, MF_STRING, MENU_FLIP as usize, if mode == Mode::On { "Turn off" } else { "Turn on" });
        add_item(menu, if muting { MF_STRING | MF_CHECKED } else { MF_STRING }, MENU_SOUND as usize, "Mute test sounds");
        add_item(menu, MF_STRING, MENU_PREVIEW as usize, "Show what runs hidden...");

        let kept = CreatePopupMenu();
        add_item(kept, MF_STRING | MF_DISABLED, 0, if listed.is_empty() {
            "Programs you add here open on your screen even while QuietDesk is on."
        } else {
            "These open on your screen even while QuietDesk is on. Click one to take it off."
        });
        AppendMenuW(kept, MF_SEPARATOR, 0, null());
        for (index, program) in listed.iter().take(MENU_PROGRAMS).enumerate() {
            let id = MENU_LISTED + index as u32;
            add_item(kept, MF_STRING | MF_CHECKED, id as usize, program);
            choices.push((id, Choice::Release(program.clone())));
        }
        if !listed.is_empty() {
            AppendMenuW(kept, MF_SEPARATOR, 0, null());
        }
        let to_add = CreatePopupMenu();
        if open.is_empty() {
            add_item(to_add, MF_STRING | MF_DISABLED, 0, "No other program has a window open");
        }
        for (index, program) in open.iter().take(MENU_PROGRAMS).enumerate() {
            let id = MENU_OPEN + index as u32;
            add_item(to_add, MF_STRING, id as usize, program);
            choices.push((id, Choice::Keep(program.clone())));
        }
        add_item(kept, MF_STRING | MF_POPUP, to_add as usize, "Add an open program");
        add_item(kept, MF_STRING, MENU_ADD_FILE as usize, "Add a program file...");
        add_item(menu, MF_STRING | MF_POPUP, kept as usize, "Always on your screen");

        AppendMenuW(menu, MF_SEPARATOR, 0, null());
        add_item(menu, MF_STRING, MENU_QUIT as usize, "Close this icon (the mode keeps working, but test sounds are no longer muted)");
        (menu, choices)
    }
}

unsafe fn show_menu(tray: HWND) {
    unsafe {
        let open = quietdesk::Desktop::real().map(|desktop| quietdesk::open_programs_on(&desktop)).unwrap_or_default();
        let (menu, choices) = build_menu(read_mode(), quietdesk::sound::muting_enabled(), &quietdesk::excluded_programs(), &open);
        let mut cursor = POINT { x: 0, y: 0 };
        GetCursorPos(&mut cursor);
        // Windows closes a tray menu on a click elsewhere only if its owner is in front; the user's window is invisible.
        SetForegroundWindow(tray);
        let chosen = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, cursor.x, cursor.y, 0, tray, null()) as u32;
        PostMessageW(tray, WM_NULL, 0, 0);
        // Its submenus go with it.
        DestroyMenu(menu);
        if let Some((_, choice)) = choices.into_iter().find(|(id, _)| *id == chosen) {
            choose(tray, choice);
        }
    }
}

unsafe fn choose(tray: HWND, choice: Choice) {
    unsafe {
        match choice {
            Choice::Flip => flip(),
            Choice::Sound => {
                if let Err(error) = quietdesk::sound::set_muting(!quietdesk::sound::muting_enabled()) {
                    warn(&format!("Could not save that: {error}"));
                }
            }
            Choice::Preview => open_preview(),
            Choice::Quit => {
                DestroyWindow(tray);
            }
            Choice::Keep(program) => keep_on_screen(&program, true),
            Choice::Release(program) => keep_on_screen(&program, false),
            Choice::KeepFile => {
                if let Some(path) = pick_program_file(tray) {
                    keep_on_screen(&path, true);
                }
            }
        }
    }
}

/// Changes the list. The shims and the keeper read it each time, so it counts from the next command.
fn keep_on_screen(program: &str, kept: bool) {
    if let Err(error) = quietdesk::set_excluded(program, kept) {
        warn(&format!("Could not change the list of programs always on your screen: {error}"));
    }
}

/// Windows' own Open dialog, for a program file; its path, or None if the user cancelled.
unsafe fn pick_program_file(owner: HWND) -> Option<String> {
    unsafe {
        let mut file = [0u16; 1024];
        let filter: Vec<u16> = "Programs (*.exe)\0*.exe\0All files (*.*)\0*.*\0\0".encode_utf16().collect();
        let title = wide("Choose a program to keep on your screen");
        let mut dialog: OPENFILENAMEW = std::mem::zeroed();
        dialog.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        dialog.hwndOwner = owner;
        dialog.lpstrFilter = filter.as_ptr();
        dialog.lpstrFile = file.as_mut_ptr();
        dialog.nMaxFile = file.len() as u32;
        dialog.lpstrTitle = title.as_ptr();
        dialog.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_DONTADDTORECENT;
        if GetOpenFileNameW(&mut dialog) == 0 {
            return None;
        }
        let length = file.iter().position(|&unit| unit == 0).unwrap_or(file.len());
        Some(String::from_utf16_lossy(&file[..length]))
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuItemID, GetMenuState, GetMenuStringW, GetSubMenu, MF_BYPOSITION};

    struct Item {
        text: String,
        id: u32,
        checked: bool,
        disabled: bool,
        sub: HMENU,
    }

    /// A menu's items as Windows holds them.
    fn items(menu: HMENU) -> Vec<Item> {
        unsafe {
            (0..GetMenuItemCount(menu))
                .map(|position| {
                    let mut text = [0u16; 256];
                    let length = GetMenuStringW(menu, position as u32, text.as_mut_ptr(), text.len() as i32, MF_BYPOSITION);
                    let state = GetMenuState(menu, position as u32, MF_BYPOSITION);
                    Item {
                        text: String::from_utf16_lossy(&text[..length.max(0) as usize]),
                        id: GetMenuItemID(menu, position),
                        checked: state & MF_CHECKED != 0,
                        disabled: state & MF_DISABLED != 0,
                        sub: GetSubMenu(menu, position),
                    }
                })
                .collect()
        }
    }

    fn named<'a>(items: &'a [Item], text: &str) -> &'a Item {
        items.iter().find(|item| item.text == text).unwrap_or_else(|| panic!("no item {text:?}"))
    }

    /// The list of programs always on the user's screen, as the tray shows it and changes it: those on it ticked, a click takes
    /// one off, the open programs and a program file put one on. Against a state folder of its own, never the user's list.
    #[test]
    fn the_tray_menu_shows_the_list_and_changes_it() {
        let state = std::env::temp_dir().join(format!("quietdesk-tray-test-{}", std::process::id()));
        std::fs::create_dir_all(&state).unwrap();
        // The only test in this program, so nothing else reads the state folder meanwhile.
        unsafe { std::env::set_var("QUIETDESK_DIR", &state) };

        let (menu, _) = unsafe { build_menu(Mode::Off, false, &[], &[]) };
        let top = items(menu);
        assert!(named(&top, "QuietDesk is OFF").disabled);
        assert_eq!(named(&top, "Turn on").id, MENU_FLIP);
        let kept = items(named(&top, "Always on your screen").sub);
        assert!(named(&kept, "Programs you add here open on your screen even while QuietDesk is on.").disabled);
        let nothing_open = items(named(&kept, "Add an open program").sub);
        assert!(nothing_open.len() == 1 && nothing_open[0].disabled, "with nothing open, it says so");
        assert_eq!(named(&kept, "Add a program file...").id, MENU_ADD_FILE);
        unsafe { DestroyMenu(menu) };

        quietdesk::set_excluded("Blender", true).unwrap();
        let listed = quietdesk::excluded_programs();
        let open = vec!["code.exe".to_string(), "notepad.exe".to_string()];
        let (menu, choices) = unsafe { build_menu(Mode::On, true, &listed, &open) };
        let top = items(menu);
        assert!(named(&top, "Mute test sounds").checked);
        let kept = items(named(&top, "Always on your screen").sub);
        assert!(named(&kept, "These open on your screen even while QuietDesk is on. Click one to take it off.").disabled);
        let blender = named(&kept, "blender.exe");
        assert!(blender.checked && !blender.disabled);
        let offered = items(named(&kept, "Add an open program").sub);
        assert_eq!(offered.iter().map(|item| item.text.as_str()).collect::<Vec<_>>(), ["code.exe", "notepad.exe"]);
        let choice = |id: u32| choices.iter().find(|(candidate, _)| *candidate == id).map(|(_, choice)| choice);
        assert_eq!(choice(blender.id), Some(&Choice::Release("blender.exe".into())));
        assert_eq!(choice(named(&offered, "notepad.exe").id), Some(&Choice::Keep("notepad.exe".into())));
        assert_eq!(choice(named(&kept, "Add a program file...").id), Some(&Choice::KeepFile));
        unsafe { DestroyMenu(menu) };

        // What the clicks do. A program file comes back from the Open dialog as its full path.
        unsafe {
            choose(null_mut(), Choice::Keep("notepad.exe".into()));
            choose(null_mut(), Choice::Release("blender.exe".into()));
            choose(null_mut(), Choice::Keep(r"C:\Program Files\Some Editor\Editor.exe".into()));
        }
        assert_eq!(quietdesk::excluded_programs(), ["editor.exe", "notepad.exe"]);
        let _ = std::fs::remove_dir_all(&state);
    }
}
