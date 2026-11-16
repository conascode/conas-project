//! GM_Menu: the ⋯ menu of a note and the right-click menu of the list window
//! (common.css .menu / .menu-item / .swatches). An owned popup placed where v1.1
//! draws its absolutely-positioned menu, with the same "compact" rule.

use crate::app;
use crate::gfx::{self, rect, ws, FontSpec};
use crate::icons::{self, Icon};
use crate::note::{WM_APP_MENU, WM_APP_MENUCLOSED};
use crate::theme::{g, mix, pal, COLOR_NAMES, EDGE, ON_ACCENT, PALETTE};
use crate::win::{px, wide, x_of, y_of};
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, SetFocus};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Rename,
    Color(usize),
    Band,
    Full,
    Lock,
    Theme,
    List,
    Delete,
    ListDelete,
}

impl Act {
    pub fn code(self) -> usize {
        match self {
            Act::Rename => 1,
            Act::Band => 2,
            Act::Full => 3,
            Act::Lock => 4,
            Act::Theme => 5,
            Act::List => 6,
            Act::Delete => 7,
            Act::ListDelete => 8,
            Act::Color(i) => 100 + i,
        }
    }
    pub fn from_code(c: usize) -> Act {
        match c {
            1 => Act::Rename,
            2 => Act::Band,
            3 => Act::Full,
            4 => Act::Lock,
            5 => Act::Theme,
            6 => Act::List,
            7 => Act::Delete,
            8 => Act::ListDelete,
            c => Act::Color(c.saturating_sub(100).min(5)),
        }
    }
    /// v1.1: colour swatches and the band row leave the menu open.
    fn keeps_open(self) -> bool {
        matches!(self, Act::Band | Act::Color(_))
    }
}

pub enum Row {
    Item { act: Act, label: String, value: Option<String>, disabled: bool },
    Swatches { selected: usize, disabled: bool },
}

struct State {
    owner: HWND,
    rows: Vec<Row>,
    compact: bool,
    min_w: f32,
    hot: Option<(usize, usize)>,
    focus: Option<(usize, usize)>,
    keyboard: bool,
    list_menu: bool,
}

const CLASS: &str = "GM_Menu";

fn register() {
    unsafe {
        let name = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DROPSHADOW * 0,
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

pub fn note_rows(id: u64, full: bool) -> Vec<Row> {
    let n = app::note(id);
    let locked = n.as_ref().map(|n| n.locked).unwrap_or(false);
    let color = n.as_ref().map(|n| n.color).unwrap_or(0);
    let item = |act, label: &str, disabled| Row::Item { act, label: label.into(), value: None, disabled };
    vec![
        item(Act::Rename, "제목 수정", locked),
        Row::Swatches { selected: color, disabled: locked },
        Row::Item { act: Act::Band, label: "띠 모양".into(), value: Some(app::band().label().into()), disabled: false },
        item(Act::Full, if full { "원래 크기" } else { "전체화면" }, false),
        item(Act::Lock, if locked { "잠금 해제" } else { "잠금" }, false),
        item(Act::Theme, if app::theme() == crate::theme::Theme::Light { "다크 모드" } else { "라이트 모드" }, false),
        item(Act::List, "메모 목록", false),
        item(Act::Delete, "메모 삭제", locked),
    ]
}

unsafe fn st<'a>(h: HWND) -> Option<&'a mut State> {
    (GetWindowLongPtrW(h, GWLP_USERDATA) as *mut State).as_mut()
}

fn row_h(r: &Row, compact: bool, dpi: u32) -> i32 {
    match r {
        Row::Item { .. } => px(if compact { g::ITEM_H_COMPACT } else { g::ITEM_H }, dpi),
        Row::Swatches { .. } => px(if compact { 2.0 + g::SWATCH + 2.0 } else { 6.0 + g::SWATCH + 6.0 }, dpi),
    }
}

/// Box size (without the 1 px outline) for the rows.
fn box_size(rows: &[Row], compact: bool, min_w: f32, dpi: u32) -> (i32, i32) {
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let f = gfx::font(FontSpec::px(130), dpi);
        let pad = px(g::ITEM_PAD_X, dpi);
        let mut w = px(min_w, dpi);
        for r in rows {
            let need = match r {
                Row::Item { label, value: Some(v), .. } => gfx::text_w(dc, &ws(label), f) + px(16.0, dpi) + gfx::text_w(dc, &ws(v), f) + 2 * pad,
                Row::Item { label, .. } => gfx::text_w(dc, &ws(label), f) + 2 * pad,
                Row::Swatches { .. } => 2 * pad + 6 * px(g::SWATCH, dpi) + 5 * px(g::SWATCH_GAP, dpi),
            };
            w = w.max(need);
        }
        ReleaseDC(std::ptr::null_mut(), dc);
        let h = 2 * px(g::MENU_PAD_Y, dpi) + rows.iter().map(|r| row_h(r, compact, dpi)).sum::<i32>();
        (w, h)
    }
}

fn create(owner: HWND, rows: Vec<Row>, min_w: f32, list_menu: bool) -> HWND {
    register();
    let s = Box::new(State { owner, rows, compact: false, min_w, hot: None, focus: None, keyboard: false, list_menu });
    unsafe {
        let h = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            wide(CLASS).as_ptr(),
            std::ptr::null(),
            WS_POPUP,
            0,
            0,
            10,
            10,
            owner,
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            std::ptr::null(),
        );
        SetWindowLongPtrW(h, GWLP_USERDATA, Box::into_raw(s) as isize);
        h
    }
}

fn first_focusable(s: &State) -> Option<(usize, usize)> {
    focusables(s).into_iter().next()
}

fn focusables(s: &State) -> Vec<(usize, usize)> {
    let mut v = Vec::new();
    for (i, r) in s.rows.iter().enumerate() {
        match r {
            Row::Item { disabled: false, .. } => v.push((i, 0)),
            Row::Swatches { disabled: false, .. } => (0..6).for_each(|k| v.push((i, k))),
            _ => {}
        }
    }
    v
}

/// The ⋯ menu of a note; placed by `reposition_note_menu`.
pub fn show_note_menu(note: HWND, id: u64, full: bool) -> HWND {
    let h = create(note, note_rows(id, full), g::MENU_MIN_W, false);
    reposition_note_menu(h, note);
    unsafe {
        if let Some(s) = st(h) {
            s.focus = first_focusable(s); // v1.1 focuses the first enabled item (no ring after a click)
        }
        if crate::quiet() {
            ShowWindow(h, SW_SHOWNOACTIVATE);
        } else {
            ShowWindow(h, SW_SHOW);
            SetFocus(h);
        }
    }
    h
}

pub fn refresh_note_menu(h: HWND, id: u64, full: bool) {
    unsafe {
        if let Some(s) = st(h) {
            s.rows = note_rows(id, full);
        }
        InvalidateRect(h, std::ptr::null(), 0);
    }
}

/// `#menu { top: var(--bar) [+4 thin]; right: 5px }`, or `.compact { top: 2px }` with 20 px rows
/// when the note is too short (F-25). The 1 px outline sits outside the box.
pub fn reposition_note_menu(h: HWND, note: HWND) {
    unsafe {
        let Some(s) = st(h) else { return };
        let dpi = GetDpiForWindow(note);
        let mut rc = rect(0, 0, 0, 0);
        GetClientRect(note, &mut rc);
        let top = px(app::band().strip() + g::BAR, dpi);
        let (_, full_h) = box_size(&s.rows, false, s.min_w, dpi);
        s.compact = rc.bottom < top + full_h + px(4.0, dpi);
        let (bw, bh) = box_size(&s.rows, s.compact, s.min_w, dpi);
        let y = if s.compact { px(2.0, dpi) } else { top };
        let mut p = POINT { x: rc.right - px(5.0, dpi) - bw, y };
        ClientToScreen(note, &mut p);
        let one = 1; // v1.1 outline renders as one device pixel at every scale
        SetWindowPos(h, HWND_TOP, p.x - one, p.y - one, bw + 2 * one, bh + 2 * one, SWP_NOACTIVATE);
        InvalidateRect(h, std::ptr::null(), 0);
    }
}

/// Right-click menu of a list row: "메모 삭제" (or a disabled "잠긴 메모예요").
pub fn show_list_menu(list: HWND, locked: bool, at: POINT) -> HWND {
    let rows = vec![Row::Item { act: Act::ListDelete, label: if locked { "잠긴 메모예요" } else { "메모 삭제" }.into(), value: None, disabled: locked }];
    let h = create(list, rows, 120.0, true);
    unsafe {
        let dpi = GetDpiForWindow(list);
        let Some(s) = st(h) else { return h };
        let (bw, bh) = box_size(&s.rows, false, s.min_w, dpi);
        let one = 1; // v1.1 outline renders as one device pixel at every scale
        let mut rc = rect(0, 0, 0, 0);
        GetClientRect(list, &mut rc);
        let mut c = at;
        ScreenToClient(list, &mut c);
        let m = px(4.0, dpi);
        let x = c.x.min(rc.right - bw - 2 * one - m).max(m);
        let y = c.y.min(rc.bottom - bh - 2 * one - m).max(m);
        let mut p = POINT { x, y };
        ClientToScreen(list, &mut p);
        SetWindowPos(h, HWND_TOP, p.x, p.y, bw + 2 * one, bh + 2 * one, SWP_NOACTIVATE);
        s.focus = first_focusable(s);
        if crate::quiet() {
            ShowWindow(h, SW_SHOWNOACTIVATE);
        } else {
            ShowWindow(h, SW_SHOW);
            SetFocus(h);
        }
    }
    h
}

fn layout_rows(s: &State, w: i32, dpi: u32) -> Vec<RECT> {
    let one = 1; // v1.1 outline renders as one device pixel at every scale
    let mut y = one + px(g::MENU_PAD_Y, dpi);
    s.rows
        .iter()
        .map(|r| {
            let h = row_h(r, s.compact, dpi);
            let rr = rect(one, y, w - one, y + h);
            y += h;
            rr
        })
        .collect()
}

fn swatch_rects(row: &RECT, dpi: u32) -> Vec<RECT> {
    let sz = px(g::SWATCH, dpi);
    let gap = px(g::SWATCH_GAP, dpi);
    let y = (row.top + row.bottom - sz) / 2;
    (0..6)
        .map(|k| {
            let x = row.left + px(g::ITEM_PAD_X, dpi) + k * (sz + gap);
            rect(x, y, x + sz, y + sz)
        })
        .collect()
}

fn hit(s: &State, h: HWND, x: i32, y: i32) -> Option<(usize, usize)> {
    let dpi = unsafe { GetDpiForWindow(h) };
    let mut rc = rect(0, 0, 0, 0);
    unsafe { GetClientRect(h, &mut rc) };
    for (i, r) in layout_rows(s, rc.right, dpi).iter().enumerate() {
        if y >= r.top && y < r.bottom && x >= r.left && x < r.right {
            return match &s.rows[i] {
                Row::Item { .. } => Some((i, 0)),
                Row::Swatches { .. } => swatch_rects(r, dpi).iter().position(|q| x >= q.left && x < q.right && y >= q.top && y < q.bottom).map(|k| (i, k)),
            };
        }
    }
    None
}

pub static PAINTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn paint(h: HWND) {
    PAINTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Some(s) = (unsafe { st(h) }) else { return };
    let dpi = unsafe { GetDpiForWindow(h) };
    let p = pal(app::theme());
    gfx::paint_buffered(h, |hdc, w, hh| {
        gfx::fill(hdc, &rect(0, 0, w, hh), p.paper);
        let f = gfx::font(FontSpec::px(130), dpi);
        let pad = px(g::ITEM_PAD_X, dpi);
        for (i, r) in layout_rows(s, w, dpi).iter().enumerate() {
            match &s.rows[i] {
                Row::Item { label, value, disabled, .. } => {
                    let hot = s.hot == Some((i, 0));
                    let focused = s.focus == Some((i, 0)) && s.keyboard;
                    if !disabled && (hot || focused) {
                        gfx::fill(hdc, r, p.paper3);
                    }
                    let c = if *disabled { p.muted } else { p.ink };
                    gfx::text(hdc, &ws(label), &rect(r.left + pad, r.top, r.right - pad, r.bottom), f, c, DT_SINGLELINE | DT_VCENTER | DT_LEFT);
                    if let Some(v) = value {
                        gfx::text(hdc, &ws(v), &rect(r.left + pad, r.top, r.right - pad, r.bottom), f, p.muted, DT_SINGLELINE | DT_VCENTER | DT_RIGHT);
                    }
                    if focused {
                        focus_ring(hdc, r, dpi, p.ink);
                    }
                }
                Row::Swatches { selected, disabled } => {
                    for (k, q) in swatch_rects(r, dpi).iter().enumerate() {
                        let grow = if !disabled && s.hot == Some((i, k)) { px(1.0, dpi) } else { 0 }; // scale(1.12)
                        let q2 = rect(q.left - grow, q.top - grow, q.right + grow, q.bottom + grow);
                        let col = if *disabled { mix(PALETTE[k], p.paper, 0.35) } else { PALETTE[k] };
                        gfx::fill(hdc, &q2, col);
                        if k == *selected {
                            let chk = if *disabled { mix(ON_ACCENT, p.paper, 0.35) } else { ON_ACCENT };
                            icons::draw_centered(hdc, Icon::SwatchCheck, &q2, 10.0, dpi, chk, false);
                        }
                        if s.keyboard && s.focus == Some((i, k)) {
                            let o = px(1.5, dpi);
                            focus_ring(hdc, &rect(q2.left - o, q2.top - o, q2.right + o, q2.bottom + o), dpi, p.ink);
                        }
                    }
                }
            }
        }
        gfx::frame(hdc, &rect(0, 0, w, hh), EDGE);
    });
}

fn focus_ring(hdc: HDC, r: &RECT, dpi: u32, c: u32) {
    let t = px(1.5, dpi).max(1);
    for k in 0..t {
        gfx::frame(hdc, &rect(r.left + k, r.top + k, r.right - k, r.bottom - k), c);
    }
}

fn activate(h: HWND, at: (usize, usize)) {
    let Some(s) = (unsafe { st(h) }) else { return };
    let act = match &s.rows[at.0] {
        Row::Item { act, disabled: false, .. } => *act,
        Row::Swatches { disabled: false, .. } => Act::Color(at.1),
        _ => return,
    };
    let owner = s.owner;
    unsafe {
        if act.keeps_open() {
            SendMessageW(owner, WM_APP_MENU, act.code(), 0);
        } else {
            DestroyWindow(h); // v1.1: closeMenu() first, then the action
            SendMessageW(owner, WM_APP_MENU, act.code(), 0);
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
        WM_MOUSEACTIVATE => MA_ACTIVATE as LRESULT,
        WM_MOUSEMOVE => {
            if let Some(s) = st(h) {
                let hot = hit(s, h, x_of(lp), y_of(lp));
                if hot != s.hot {
                    s.hot = hot;
                    InvalidateRect(h, std::ptr::null(), 0);
                }
                crate::note::track(h, false);
            }
            0
        }
        0x02A3 /* WM_MOUSELEAVE */ => {
            if let Some(s) = st(h) {
                s.hot = None;
                InvalidateRect(h, std::ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(s) = st(h) {
                s.keyboard = false;
                if let Some(at) = hit(s, h, x_of(lp), y_of(lp)) {
                    activate(h, at);
                }
            }
            0
        }
        WM_KEYDOWN => {
            let Some(s) = st(h) else { return 0 };
            let list = focusables(s);
            let cur = s.focus.and_then(|f| list.iter().position(|&x| x == f));
            let shift = GetKeyState(VK_SHIFT as i32) < 0;
            match wp as u16 {
                VK_ESCAPE => {
                    let owner = s.owner;
                    DestroyWindow(h);
                    SetForegroundWindow(owner);
                }
                VK_DOWN | VK_RIGHT | VK_TAB | VK_UP | VK_LEFT if !list.is_empty() => {
                    let back = wp as u16 == VK_UP || wp as u16 == VK_LEFT || (wp as u16 == VK_TAB && shift);
                    let n = list.len();
                    let i = match cur {
                        None => 0,
                        Some(i) if back => (i + n - 1) % n,
                        Some(i) => (i + 1) % n,
                    };
                    s.focus = Some(list[i]);
                    s.keyboard = true;
                    InvalidateRect(h, std::ptr::null(), 0);
                }
                VK_RETURN | VK_SPACE => {
                    if let Some(f) = s.focus {
                        s.keyboard = true;
                        activate(h, f);
                    }
                }
                _ => {}
            }
            0
        }
        WM_ACTIVATE => {
            if (wp & 0xFFFF) as u32 == WA_INACTIVE {
                // clicking anywhere else closes the menu (document mousedown / window blur)
                PostMessageW(h, WM_CLOSE, 0, 0);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(h);
            0
        }
        WM_DESTROY => {
            if let Some(s) = st(h) {
                PostMessageW(s.owner, WM_APP_MENUCLOSED, 0, 0);
                let _ = s.list_menu;
            }
            0
        }
        WM_NCDESTROY => {
            let p = SetWindowLongPtrW(h, GWLP_USERDATA, 0) as *mut State;
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            0
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

#[allow(dead_code)]
pub fn color_name(i: usize) -> &'static str {
    COLOR_NAMES[i % 6]
}
