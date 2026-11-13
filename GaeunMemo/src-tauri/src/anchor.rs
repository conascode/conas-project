//! A hidden plain Win32 window (no webview) that receives session-end
//! messages (logoff / shutdown / restart) so notes are saved even when no
//! note window is open. Note windows now have their own taskbar entries, so
//! this window is never shown.

use std::sync::OnceLock;
use tauri::AppHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

static APP: OnceLock<AppHandle> = OnceLock::new();

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn create(app: &AppHandle) {
    let _ = APP.set(app.clone());
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("gaeunmemo_taskbar");
        let title = wide("GaeunMemo");
        // tauri-build embeds the app icon as resource 32512
        let mut icon = LoadIconW(hinst, 32512usize as *const u16);
        if icon.is_null() {
            icon = LoadIconW(std::ptr::null_mut(), IDI_APPLICATION);
        }
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: icon,
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
            hIconSm: icon,
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
            -32000,
            -32000,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinst,
            std::ptr::null(),
        );
        // stays hidden: hidden top-level windows still get WM_QUERYENDSESSION
        if hwnd.is_null() {
            crate::log!("session window: create failed");
        }
    }
}

// ---------- top edge: drag instead of resize ----------
//
// Tauri covers a frameless window's border ring with a child window
// ("TAURI_DRAG_RESIZE_WINDOW") that turns clicks into resizes. We cut the
// middle of the top edge out of that child's region so the band underneath
// receives the mouse and can drag the window. Tauri resets the region on every
// WM_SIZE, so a subclass re-applies the cut afterwards.

use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, DeleteObject, SetWindowRgn, RGN_DIFF};
use windows_sys::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};

const EDGE_SUBCLASS: usize = 0x6761_656E; // "gaen"

pub fn free_top_edge(hwnd: isize) {
    let parent = hwnd as HWND;
    unsafe {
        SetWindowSubclass(parent, Some(edge_proc), EDGE_SUBCLASS, 0);
        cut_top(parent);
    }
}

unsafe extern "system" fn edge_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let r = DefSubclassProc(hwnd, msg, wparam, lparam);
    match msg {
        WM_SIZE | WM_DPICHANGED => cut_top(hwnd),
        WM_GETMINMAXINFO => {
            // Windows' own minimum height (~37px) is taller than a folded note
            // without a band. Lower it, but keep any larger minimum the app set.
            let info = lparam as *mut MINMAXINFO;
            if !info.is_null() {
                let sys_min = GetSystemMetricsForDpi(SM_CYMINTRACK, GetDpiForWindow(hwnd));
                if (*info).ptMinTrackSize.y <= sys_min {
                    (*info).ptMinTrackSize.y = 1;
                }
            }
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(hwnd, Some(edge_proc), EDGE_SUBCLASS);
        }
        _ => {}
    }
    r
}

unsafe fn cut_top(parent: HWND) {
    let class = wide("TAURI_DRAG_RESIZE_BORDERS");
    let name = wide("TAURI_DRAG_RESIZE_WINDOW");
    let child = FindWindowExW(parent, std::ptr::null_mut(), class.as_ptr(), name.as_ptr());
    if child.is_null() {
        return;
    }
    let mut rc: RECT = std::mem::zeroed();
    if GetClientRect(parent, &mut rc) == 0 {
        return;
    }
    let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
    if w <= 0 || h <= 0 {
        return;
    }
    let dpi = GetDpiForWindow(parent);
    let bx = GetSystemMetricsForDpi(SM_CXFRAME, dpi);
    let by = GetSystemMetricsForDpi(SM_CYFRAME, dpi);
    let ring = CreateRectRgn(0, 0, w, h);
    let inner = CreateRectRgn(bx, by, w - bx, h - by);
    CombineRgn(ring, ring, inner, RGN_DIFF);
    DeleteObject(inner);
    // keep the top corners for diagonal resizing
    let top = CreateRectRgn(bx * 2, 0, w - bx * 2, by);
    CombineRgn(ring, ring, top, RGN_DIFF);
    DeleteObject(top);
    if SetWindowRgn(child, ring, 1) == 0 {
        DeleteObject(ring);
    }
}

fn on_main(f: impl FnOnce(&AppHandle) + Send + 'static) {
    if let Some(app) = APP.get() {
        let a = app.clone();
        let _ = app.run_on_main_thread(move || f(&a));
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_SYSCOMMAND => {
            let cmd = (wparam & 0xFFF0) as u32;
            if cmd == SC_CLOSE {
                on_main(crate::quit);
                return 0;
            }
        }
        WM_QUERYENDSESSION => {
            crate::log!("session ending: saving");
            crate::flush_now();
            return 1;
        }
        WM_ENDSESSION => {
            if wparam != 0 {
                crate::flush_now();
                crate::log!("EXIT: session ended");
            }
            return 0;
        }
        _ => {}
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}
