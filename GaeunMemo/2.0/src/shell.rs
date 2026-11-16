//! Shell integration (report §6.5, F-16, F-18, session end): one hidden top-level window
//! that owns the tray icon and its menu, gets `TaskbarCreated` (Explorer restarted: the icon
//! comes back), the session-end messages (saved even when no note is open — v1.1 anchor.rs)
//! and the "show yourself" request of a second launch (F-18).
//!
//! The window is never shown. A hidden top-level window still gets WM_QUERYENDSESSION and
//! registered broadcast messages; a message-only window would not.

use crate::app;
use crate::autostart;
use crate::list;
use crate::win::wide;
use std::cell::Cell;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS: &str = "GaeunMemo2Session";
const WM_APP_TRAY: u32 = WM_APP + 30;
const TRAY_ID: u32 = 1;
const CMD_NEW: usize = 1;
const CMD_LIST: usize = 2;
const CMD_AUTO: usize = 3;
const CMD_QUIT: usize = 4;
/// NOTIFYICON_VERSION_4 events: a left click / Enter on the icon (WM_USER + 0 / + 1)
const NIN_SELECT_: u32 = WM_USER;
const NIN_KEYSELECT_: u32 = WM_USER + 1;

thread_local! {
    static SESSION: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    static TRAY: Cell<bool> = const { Cell::new(false) };
}

/// The data folder of this process (`--data`), for the autostart command line.
static DATA: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();

fn taskbar_created() -> u32 {
    static M: OnceLock<u32> = OnceLock::new();
    *M.get_or_init(|| unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) })
}

fn show_request() -> u32 {
    static M: OnceLock<u32> = OnceLock::new();
    *M.get_or_init(|| unsafe { RegisterWindowMessageW(wide("GaeunMemo2.ShowAll").as_ptr()) })
}

/// The session window's title names the data folder, so a second launch on the same folder
/// finds this process (tests run other folders side by side).
fn title_for(dir: &std::path::Path) -> String {
    format!("GaeunMemo2 {}", dir.to_string_lossy().to_lowercase())
}

/// The app icon (resource 1, v1.1's icon.ico) at a given size.
pub fn app_icon(size: i32) -> HICON {
    unsafe { LoadImageW(GetModuleHandleW(std::ptr::null()), 1 as _, IMAGE_ICON, size, size, LR_DEFAULTCOLOR) as HICON }
}

/// A second launch on the same data folder: asks the running app to show its notes and lets
/// it take the foreground. False when no running app answers (the caller then just ends).
pub fn signal_running(dir: &std::path::Path) -> bool {
    unsafe {
        let h = FindWindowW(wide(CLASS).as_ptr(), wide(&title_for(dir)).as_ptr());
        if h.is_null() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, &mut pid);
        AllowSetForegroundWindow(pid);
        PostMessageW(h, show_request(), 0, 0) != 0
    }
}

/// Creates the session window and the tray icon. A tray failure is logged and the app goes
/// on (v1.1: "트레이 생성 실패해도 앱은 계속").
pub fn create(dir: &std::path::Path, data: Option<std::path::PathBuf>) -> HWND {
    create_with(dir, data, true)
}

/// `create`; `tray: false` leaves the tray alone (tests: no icon on the user's taskbar).
pub fn create_with(dir: &std::path::Path, data: Option<std::path::PathBuf>, tray: bool) -> HWND {
    let _ = DATA.set(data);
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: app_icon(32),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: app_icon(16),
        };
        RegisterClassExW(&wc);
        let h = CreateWindowExW(WS_EX_TOOLWINDOW, class.as_ptr(), wide(&title_for(dir)).as_ptr(), WS_POPUP, -32000, -32000, 1, 1, std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null());
        if h.is_null() {
            crate::log::write("session window: create failed");
            return h;
        }
        SESSION.with(|s| s.set(h));
        let _ = taskbar_created();
        // Explorer runs at a higher integrity level than nothing here, but an elevated
        // Explorer would filter the broadcast: let it through
        ChangeWindowMessageFilterEx(h, taskbar_created(), MSGFLT_ALLOW, std::ptr::null_mut());
        if tray && !add_tray(h) {
            crate::log::write("tray: create failed (the app goes on without it)");
        }
        h
    }
}

/// Requests a second launch made (tests).
pub static SHOW_REQUESTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn tray_data(h: HWND) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = h;
    // an id, not a GUID: a GUID icon is tied to one exe path, and development copies run
    // from several (dist\, target\release\) — the installer build can switch (P7)
    d.uID = TRAY_ID;
    d
}

fn add_tray(h: HWND) -> bool {
    unsafe {
        let mut d = tray_data(h);
        d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        d.uCallbackMessage = WM_APP_TRAY;
        let dpi = windows_sys::Win32::UI::HiDpi::GetDpiForSystem();
        d.hIcon = app_icon(windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi(SM_CXSMICON, dpi));
        for (i, c) in "GaeunMemo".encode_utf16().enumerate() {
            d.szTip[i] = c;
        }
        let ok = Shell_NotifyIconW(NIM_ADD, &d) != 0;
        if ok {
            d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &d);
        }
        TRAY.with(|t| t.set(ok));
        ok
    }
}

/// Removes the tray icon (program end), so no dead icon stays until the mouse passes over.
pub fn remove_tray() {
    let h = SESSION.with(|s| s.get());
    if h.is_null() || !TRAY.with(|t| t.get()) {
        return;
    }
    unsafe {
        let d = tray_data(h);
        Shell_NotifyIconW(NIM_DELETE, &d);
    }
    TRAY.with(|t| t.set(false));
}

/// v1.1 `show_all`: every open note to the front; with none open, the list.
pub fn show_all(focus: bool) {
    let ids: Vec<u64> = app::with(|a| a.notes.iter().filter(|n| n.open).map(|n| n.id).collect());
    if ids.is_empty() {
        list::open(focus);
        return;
    }
    if list::is_open() {
        list::open(false);
    }
    let last = ids.len() - 1;
    for (k, id) in ids.into_iter().enumerate() {
        app::open_note(id, None, focus && k == last);
    }
}

/// Tray 종료: everything typed is saved, then the message loop ends (main closes the store).
/// Open notes stay open in the store and come back at the next start (v1.1).
pub fn quit() {
    crate::log::write("EXIT: 종료 (tray)");
    crate::note::flush_all();
    remove_tray();
    unsafe { PostQuitMessage(0) };
}

fn menu(h: HWND) {
    unsafe {
        let m = CreatePopupMenu();
        if m.is_null() {
            return;
        }
        AppendMenuW(m, MF_STRING, CMD_NEW, wide("새 메모").as_ptr());
        AppendMenuW(m, MF_STRING, CMD_LIST, wide("메모 목록").as_ptr());
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        let auto = if autostart::is_enabled() { MF_CHECKED } else { MF_UNCHECKED };
        AppendMenuW(m, MF_STRING | auto, CMD_AUTO, wide("컴퓨터 켤 때 자동 실행").as_ptr());
        AppendMenuW(m, MF_SEPARATOR, 0, std::ptr::null());
        AppendMenuW(m, MF_STRING, CMD_QUIT, wide("종료").as_ptr());
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        // the menu closes when the user clicks elsewhere only if its owner is in front
        SetForegroundWindow(h);
        let cmd = TrackPopupMenuEx(m, (TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RIGHTALIGN) as u32, pt.x, pt.y, h, std::ptr::null()) as usize;
        PostMessageW(h, WM_NULL, 0, 0);
        DestroyMenu(m);
        match cmd {
            CMD_NEW => {
                app::new_note(None);
            }
            CMD_LIST => list::open(true),
            CMD_AUTO => {
                let on = !autostart::is_enabled();
                let data = DATA.get().cloned().flatten();
                if !autostart::set_enabled(on, data.as_deref()) {
                    crate::log::write("autostart: registry write failed");
                }
                crate::store::send(crate::store::Cmd::Setting("autostart".into(), if autostart::is_enabled() { "1" } else { "0" }.into()));
            }
            CMD_QUIT => quit(),
            _ => {}
        }
    }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_APP_TRAY => {
            // NOTIFYICON_VERSION_4: the event is LOWORD(lParam)
            match (lp & 0xFFFF) as u32 {
                NIN_SELECT_ | NIN_KEYSELECT_ => show_all(true),
                WM_CONTEXTMENU => menu(h),
                _ => {}
            }
            0
        }
        WM_QUERYENDSESSION => {
            // log off / shut down: what is typed is committed before Windows may end us
            crate::log::write("session end: saving");
            crate::note::flush_all();
            crate::store::flush();
            1
        }
        WM_ENDSESSION => {
            if wp != 0 {
                crate::log::write("EXIT: session ended");
                remove_tray();
            }
            0
        }
        _ if msg == taskbar_created() && msg != 0 => {
            // Explorer restarted: the tray is new and empty
            TRAY.with(|t| t.set(false));
            if !add_tray(h) {
                crate::log::write("tray: re-create after Explorer restart failed");
            }
            0
        }
        _ if msg == show_request() && msg != 0 => {
            SHOW_REQUESTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            show_all(true);
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
