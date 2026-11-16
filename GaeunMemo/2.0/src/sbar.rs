//! The scroll bar of v1.1 (`common.css`: `::-webkit-scrollbar { width: 8px }`, transparent
//! track, `::-webkit-scrollbar-thumb { background: var(--color-rule) }`): 8 CSS px wide, a
//! square thumb in the rule colour straight on the paper, no arrows, no round corners.
//! A child window that sits over the right edge of a scrolling window (the note body or
//! the list rows) and mirrors its range; dragging it scrolls the target.

use crate::app;
use crate::theme::pal;
use crate::win::{px, wide};
use std::sync::Once;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const WIDTH: f32 = 8.0;
/// Chromium keeps a thumb at least this long (CSS px) however long the content is.
const MIN_THUMB: f32 = 20.0;
const CLASS: &str = "GaeunMemoScrollBar";
/// Sent to a `Target::Rows` window: lParam = the new scroll position in px.
pub const WM_APP_SCROLLTO: u32 = WM_APP + 20;

#[derive(Clone, Copy)]
pub enum Target {
    /// a RichEdit: EM_SETSCROLLPOS
    Rich(HWND),
    /// the list rows: `WM_APP_SCROLLTO`
    Rows(HWND),
}

struct Bar {
    target: Target,
    total: i32,
    page: i32,
    pos: i32,
    /// distance from the top of the thumb to where it was grabbed
    grab: Option<i32>,
}

static REG: Once = Once::new();

fn register() {
    REG.call_once(|| unsafe {
        let name = wide(CLASS);
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
        };
        RegisterClassW(&wc);
    });
}

/// A scroll bar over `parent` for `target`; hidden until `set` says there is something to scroll.
pub fn create(parent: HWND, target: Target) -> HWND {
    register();
    let bar = Box::into_raw(Box::new(Bar { target, total: 0, page: 0, pos: 0, grab: None }));
    unsafe {
        let h = CreateWindowExW(0, wide(CLASS).as_ptr(), std::ptr::null(), WS_CHILD | WS_CLIPSIBLINGS, 0, 0, 1, 1, parent, std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), bar as *const _);
        if h.is_null() {
            drop(Box::from_raw(bar));
        }
        h
    }
}

unsafe fn state<'a>(h: HWND) -> Option<&'a mut Bar> {
    let p = GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Bar;
    p.as_mut()
}

/// Mirrors the target's range: `total` and `page` in px, `pos` the top of the view.
/// Shows the bar when the content is longer than the view, hides it otherwise.
pub fn set(h: HWND, total: i32, page: i32, pos: i32) {
    if h.is_null() {
        return;
    }
    unsafe {
        let Some(b) = state(h) else { return };
        let need = page > 0 && total > page;
        let changed = b.total != total || b.page != page || b.pos != pos;
        b.total = total;
        b.page = page;
        b.pos = pos;
        let shown = IsWindowVisible(h) != 0;
        if need && !shown {
            ShowWindow(h, SW_SHOWNA);
        } else if !need && shown {
            ShowWindow(h, SW_HIDE);
        } else if need && changed {
            InvalidateRect(h, std::ptr::null(), 0);
        }
    }
}

/// Hides the bar although the target has a range (the target is hidden itself).
pub fn hide(h: HWND) {
    if !h.is_null() {
        unsafe { ShowWindow(h, SW_HIDE) };
    }
}

/// (top, length) of the thumb in a track of `track` px.
fn thumb(b: &Bar, track: i32, dpi: u32) -> (i32, i32) {
    let total = b.total.max(1) as f64;
    let min = px(MIN_THUMB, dpi).min(track);
    let len = ((track as f64 * b.page as f64 / total).round() as i32).clamp(min, track);
    let range = (b.total - b.page).max(1) as f64;
    let top = ((track - len) as f64 * (b.pos as f64 / range).clamp(0.0, 1.0)).round() as i32;
    (top, len)
}

/// Scrolls the target to `pos` and shows the result at once. While the thumb is dragged the
/// mouse keeps the queue busy, and WM_PAINT — sent only when nothing else waits — came in
/// bursts, so the text and the thumb moved in jumps: both are painted here, every move.
fn scroll_to(h: HWND, pos: i32) {
    // copies only: the target's answer updates this bar's state again (sync_bar → set)
    let Some((target, total, page, cur)) = (unsafe { state(h) }).map(|b| (b.target, b.total, b.page, b.pos)) else { return };
    let pos = pos.clamp(0, (total - page).max(0));
    if pos == cur {
        return;
    }
    unsafe {
        let t = match target {
            Target::Rich(t) => {
                let p = POINT { x: 0, y: pos };
                SendMessageW(t, 0x04DE /* EM_SETSCROLLPOS */, 0, &p as *const _ as isize);
                // the thumb follows the body's own position (it may round)
                crate::body::sync_bar(t);
                t
            }
            Target::Rows(t) => {
                SendMessageW(t, WM_APP_SCROLLTO, 0, pos as isize);
                t
            }
        };
        UpdateWindow(t);
        UpdateWindow(h);
    }
}

fn paint(h: HWND, dc: HDC) {
    unsafe {
        let Some(b) = state(h) else { return };
        let mut rc: RECT = std::mem::zeroed();
        GetClientRect(h, &mut rc);
        let p = pal(app::theme());
        let paper = CreateSolidBrush(p.paper);
        FillRect(dc, &rc, paper);
        DeleteObject(paper);
        if b.total > b.page && b.page > 0 {
            let (top, len) = thumb(b, rc.bottom, GetDpiForWindow(h).max(96));
            let r = RECT { left: 0, top, right: rc.right, bottom: top + len };
            let br = CreateSolidBrush(p.rule);
            FillRect(dc, &r, br);
            DeleteObject(br);
        }
    }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let cs = &*(lp as *const CREATESTRUCTW);
            SetWindowLongPtrW(h, GWLP_USERDATA, cs.lpCreateParams as isize);
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_NCDESTROY => {
            let p = SetWindowLongPtrW(h, GWLP_USERDATA, 0) as *mut Bar;
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(h, &mut ps);
            paint(h, dc);
            EndPaint(h, &ps);
            0
        }
        WM_PRINTCLIENT => {
            paint(h, wp as HDC);
            0
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        // the outer pixels along the window edge resize the window, not scroll
        WM_NCHITTEST if crate::win::on_parent_edge(h, lp) => HTTRANSPARENT as LRESULT,
        WM_LBUTTONDOWN => {
            let Some(b) = state(h) else { return 0 };
            let mut rc: RECT = std::mem::zeroed();
            GetClientRect(h, &mut rc);
            let y = (lp >> 16) as i16 as i32;
            let (top, len) = thumb(b, rc.bottom, GetDpiForWindow(h).max(96));
            if y >= top && y < top + len {
                b.grab = Some(y - top);
                SetCapture(h);
            } else {
                // a click on the track: one page towards it (as Chromium's first step)
                let to = if y < top { b.pos - b.page } else { b.pos + b.page };
                scroll_to(h, to);
            }
            0
        }
        WM_MOUSEMOVE => {
            let Some(b) = state(h) else { return 0 };
            if let Some(grab) = b.grab {
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(h, &mut rc);
                let (_, len) = thumb(b, rc.bottom, GetDpiForWindow(h).max(96));
                let y = (lp >> 16) as i16 as i32;
                let free = (rc.bottom - len).max(1) as f64;
                let frac = ((y - grab) as f64 / free).clamp(0.0, 1.0);
                let to = (frac * (b.total - b.page).max(0) as f64).round() as i32;
                scroll_to(h, to);
            }
            0
        }
        WM_LBUTTONUP | WM_CAPTURECHANGED => {
            if let Some(b) = state(h) {
                if b.grab.take().is_some() && GetCapture() == h {
                    ReleaseCapture();
                }
            }
            0
        }
        WM_MOUSEWHEEL => {
            // the wheel over the bar scrolls the target like the wheel over the text
            if let Some(b) = state(h) {
                let t = match b.target {
                    Target::Rich(t) | Target::Rows(t) => t,
                };
                return SendMessageW(t, msg, wp, lp);
            }
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
