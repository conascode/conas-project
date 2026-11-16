//! "삭제하시겠습니까?" card (common.css .confirm / .confirm-card / .btn): a child
//! window over the whole client, scrim = snapshot of the window blended 78 % with paper.
//! Default focus 유지; Esc and a click outside the card keep the note.

use crate::app;
use crate::gfx::{self, rect, ws, FontSpec};
use crate::note::WM_APP_CONFIRM;
use crate::theme::{g, mix, pal, EDGE, SCRIM};
use crate::win::{px, wide, x_of, y_of};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::Storage::Xps::PrintWindow;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS: &str = "GM_Confirm";

struct State {
    owner: HWND,
    id: u64,
    snapshot: HBITMAP,
    focus: usize, // 0 = 유지, 1 = 삭제
    keyboard: bool,
    hot: Option<usize>,
    down: Option<usize>,
}

fn register() {
    unsafe {
        let name = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(proc_),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: GetModuleHandleW(std::ptr::null()),
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        RegisterClassExW(&wc);
    }
}

/// Snapshot of the owner's client area with the 78 % paper scrim baked in.
fn scrim_snapshot(owner: HWND, w: i32, h: i32) -> HBITMAP {
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w.max(1), h.max(1));
        let old = SelectObject(mem, bmp);
        PrintWindow(owner, mem, 1 /* PW_CLIENTONLY: WM_PRINT through our own paint code */);
        // blend a paper-coloured 1×1 bitmap over it at 78 %
        let src = CreateCompatibleDC(screen);
        let one = CreateCompatibleBitmap(screen, 1, 1);
        let o2 = SelectObject(src, one);
        SetPixel(src, 0, 0, pal(app::theme()).paper);
        let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: (SCRIM * 255.0).round() as u8, AlphaFormat: 0 };
        GdiAlphaBlend(mem, 0, 0, w, h, src, 0, 0, 1, 1, bf);
        SelectObject(src, o2);
        DeleteObject(one);
        DeleteDC(src);
        SelectObject(mem, old);
        DeleteDC(mem);
        ReleaseDC(std::ptr::null_mut(), screen);
        bmp
    }
}

pub fn show(owner: HWND, id: u64) -> HWND {
    register();
    unsafe {
        let mut rc = rect(0, 0, 0, 0);
        GetClientRect(owner, &mut rc);
        let snapshot = scrim_snapshot(owner, rc.right, rc.bottom);
        let s = Box::new(State { owner, id, snapshot, focus: 0, keyboard: false, hot: None, down: None });
        let h = CreateWindowExW(0, wide(CLASS).as_ptr(), std::ptr::null(), WS_CHILD | WS_CLIPSIBLINGS, 0, 0, rc.right, rc.bottom, owner, std::ptr::null_mut(), GetModuleHandleW(std::ptr::null()), std::ptr::null());
        SetWindowLongPtrW(h, GWLP_USERDATA, Box::into_raw(s) as isize);
        SetWindowPos(h, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOACTIVATE);
        if !crate::quiet() {
            SetFocus(h);
        }
        h
    }
}

unsafe fn st<'a>(h: HWND) -> Option<&'a mut State> {
    (GetWindowLongPtrW(h, GWLP_USERDATA) as *mut State).as_mut()
}

struct Lay {
    card: RECT,
    text: RECT,
    keep: RECT,
    del: RECT,
}

fn lay(h: HWND, dc: HDC) -> Lay {
    let dpi = unsafe { GetDpiForWindow(h) };
    let mut rc = rect(0, 0, 0, 0);
    unsafe { GetClientRect(h, &mut rc) };
    let pad = px(12.0, dpi);
    let w = (rc.right - 2 * pad).min(px(g::CARD_MAX_W, dpi));
    let text_h = (13.0 * 1.55 * dpi as f32 / 96.0).round() as i32;
    let btn_h = px(26.0, dpi);
    let h = pad + text_h + px(12.0, dpi) + btn_h + px(10.0, dpi);
    let x = (rc.right - w) / 2;
    let y = (rc.bottom - h) / 2;
    let card = rect(x, y, x + w, y + h);
    let bf = gfx::font(FontSpec::px(125), dpi);
    let bfb = gfx::font(FontSpec::bold(125), dpi);
    let kw = gfx::text_w(dc, &ws("유지"), bf) + 2 * pad;
    let dw = gfx::text_w(dc, &ws("삭제"), bfb) + 2 * pad;
    let by = card.bottom - px(10.0, dpi) - btn_h;
    let del = rect(card.right - pad - dw, by, card.right - pad, by + btn_h);
    let keep = rect(del.left - px(6.0, dpi) - kw, by, del.left - px(6.0, dpi), by + btn_h);
    let text = rect(card.left + pad, card.top + pad, card.right - pad, card.top + pad + text_h);
    Lay { card, text, keep, del }
}

fn paint(h: HWND) {
    let Some(s) = (unsafe { st(h) }) else { return };
    let dpi = unsafe { GetDpiForWindow(h) };
    let p = pal(app::theme());
    gfx::paint_buffered(h, |dc, w, hh| unsafe {
        let src = CreateCompatibleDC(dc);
        let old = SelectObject(src, s.snapshot);
        BitBlt(dc, 0, 0, w, hh, src, 0, 0, SRCCOPY);
        SelectObject(src, old);
        DeleteDC(src);
        let l = lay(h, dc);
        gfx::fill(dc, &l.card, p.paper);
        gfx::frame(dc, &l.card, EDGE);
        gfx::text(dc, &ws("삭제하시겠습니까?"), &l.text, gfx::font(FontSpec::px(130), dpi), p.ink, DT_SINGLELINE | DT_VCENTER | DT_LEFT);
        // 유지: 1 px rule border; 삭제: strong fill, bold
        let keep_bg = if s.hot == Some(0) { if s.down == Some(0) { p.rule } else { p.paper3 } } else { p.paper };
        gfx::fill(dc, &l.keep, keep_bg);
        gfx::frame(dc, &l.keep, p.rule);
        gfx::text(dc, &ws("유지"), &l.keep, gfx::font(FontSpec::px(125), dpi), p.ink, DT_SINGLELINE | DT_VCENTER | DT_CENTER);
        let del_bg = if s.hot == Some(1) { mix(p.strong, p.paper, 0.85) } else { p.strong };
        gfx::fill(dc, &l.del, del_bg);
        gfx::text(dc, &ws("삭제"), &l.del, gfx::font(FontSpec::bold(125), dpi), p.on_strong, DT_SINGLELINE | DT_VCENTER | DT_CENTER);
        if s.keyboard {
            let r = if s.focus == 0 { l.keep } else { l.del };
            let o = px(2.0, dpi);
            let t = px(1.5, dpi).max(1);
            for k in 0..t {
                gfx::frame(dc, &rect(r.left - o - k, r.top - o - k, r.right + o + k, r.bottom + o + k), p.ink);
            }
        }
        // the window's 1 px black edge stays on top of the scrim (html::after, z-index 50)
        gfx::frame(dc, &rect(0, 0, w, hh), EDGE);
    });
}

fn finish(h: HWND, delete: bool) {
    let Some(s) = (unsafe { st(h) }) else { return };
    let (owner, id) = (s.owner, s.id);
    unsafe {
        DestroyWindow(h);
        PostMessageW(owner, WM_APP_CONFIRM, delete as usize, id as isize);
    }
}

fn btn_at(h: HWND, x: i32, y: i32) -> Option<usize> {
    unsafe {
        let dc = GetDC(h);
        let l = lay(h, dc);
        ReleaseDC(h, dc);
        let inr = |r: &RECT| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        if inr(&l.keep) {
            Some(0)
        } else if inr(&l.del) {
            Some(1)
        } else if inr(&l.card) {
            Some(9)
        } else {
            None
        }
    }
}

unsafe extern "system" fn proc_(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            paint(h);
            0
        }
        WM_PRINTCLIENT => {
            gfx::print_into(wp as HDC, || paint(h));
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            if let Some(s) = st(h) {
                let b = btn_at(h, x_of(lp), y_of(lp)).filter(|&b| b < 2);
                if b != s.hot {
                    s.hot = b;
                    InvalidateRect(h, std::ptr::null(), 0);
                }
            }
            0
        }
        WM_LBUTTONDOWN => {
            match btn_at(h, x_of(lp), y_of(lp)) {
                None => finish(h, false), // mousedown on the scrim = 유지
                Some(b) if b < 2 => {
                    if let Some(s) = st(h) {
                        s.down = Some(b);
                        s.keyboard = false;
                        InvalidateRect(h, std::ptr::null(), 0);
                    }
                }
                _ => {}
            }
            0
        }
        WM_LBUTTONUP => {
            let down = st(h).and_then(|s| s.down.take());
            if let Some(b) = down {
                if btn_at(h, x_of(lp), y_of(lp)) == Some(b) {
                    finish(h, b == 1);
                }
            }
            0
        }
        WM_GETDLGCODE => DLGC_WANTALLKEYS as LRESULT,
        WM_KEYDOWN => {
            if let Some(s) = st(h) {
                match wp as u16 {
                    VK_ESCAPE => finish(h, false),
                    VK_RETURN | VK_SPACE => finish(h, s.focus == 1),
                    VK_TAB | VK_LEFT | VK_RIGHT => {
                        let _ = GetKeyState(VK_SHIFT as i32);
                        s.focus = 1 - s.focus;
                        s.keyboard = true;
                        InvalidateRect(h, std::ptr::null(), 0);
                    }
                    _ => {}
                }
            }
            0
        }
        WM_NCDESTROY => {
            let p = SetWindowLongPtrW(h, GWLP_USERDATA, 0) as *mut State;
            if !p.is_null() {
                let s = Box::from_raw(p);
                DeleteObject(s.snapshot);
            }
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

/// True while a confirm card covers `owner` (shortcuts are ignored then, like v1.1).
pub fn is_showing(owner: HWND) -> bool {
    unsafe { !FindWindowExW(owner, std::ptr::null_mut(), wide(CLASS).as_ptr(), std::ptr::null()).is_null() }
}

/// The note was resized under the card (v1.1's overlay is a CSS layer that always
/// covers the window): follow the new client size and re-take the scrim snapshot.
/// WS_VISIBLE is dropped from the card's style while the owner is printed, so the
/// snapshot shows the note, not the card, without hiding/showing (no flicker).
pub fn resize(owner: HWND) {
    unsafe {
        let h = FindWindowExW(owner, std::ptr::null_mut(), wide(CLASS).as_ptr(), std::ptr::null());
        let Some(s) = st(h) else { return };
        let mut rc = rect(0, 0, 0, 0);
        GetClientRect(owner, &mut rc);
        let style = GetWindowLongW(h, GWL_STYLE);
        SetWindowLongW(h, GWL_STYLE, style & !(WS_VISIBLE as i32));
        let snap = scrim_snapshot(owner, rc.right, rc.bottom);
        SetWindowLongW(h, GWL_STYLE, style);
        DeleteObject(std::mem::replace(&mut s.snapshot, snap));
        SetWindowPos(h, std::ptr::null_mut(), 0, 0, rc.right, rc.bottom, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
        InvalidateRect(h, std::ptr::null(), 0);
    }
}
