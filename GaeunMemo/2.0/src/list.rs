//! GM_List: the note list window (v1.1 list.html / list.css / list.js, F-15).

use crate::app;
use crate::confirm;
use crate::gfx::{self, rect, ws, FontSpec};
use crate::icons::{self, Icon};
use crate::menu::{self, Act};
use crate::note::{self, WM_APP_CONFIRM, WM_APP_MENU, WM_APP_MENUCLOSED};
use crate::theme::{g, mix, pal, Band, EDGE, PALETTE};
use crate::win::{px, wide, x_of, y_of};
use std::cell::Cell;
use windows_sys::Win32::Foundation::*;
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{EM_SETMARGINS, EM_SETSEL};
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_DELETE, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const CLASS: &str = "GM_List";
const ROWS: &str = "GM_Rows";
const TIMER_AGO: usize = 1;

thread_local! {
    static LIST: Cell<HWND> = const { Cell::new(std::ptr::null_mut()) };
    /// where the list was last time (logical px), from the store
    static SAVED: Cell<Option<(f64, f64, f64, f64)>> = const { Cell::new(None) };
}

pub fn set_saved(rect: Option<(f64, f64, f64, f64)>) {
    SAVED.with(|s| s.set(rect));
}

/// The list window's rectangle in logical px.
fn logical_rect(h: HWND) -> Option<(f64, f64, f64, f64)> {
    unsafe {
        let mut r = rect(0, 0, 0, 0);
        if IsIconic(h) != 0 {
            // minimised from its taskbar button: the place it comes back to, not the
            // off-screen icon position
            let mut wp: WINDOWPLACEMENT = std::mem::zeroed();
            wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
            if GetWindowPlacement(h, &mut wp) == 0 {
                return None;
            }
            r = wp.rcNormalPosition;
        } else if GetWindowRect(h, &mut r) == 0 {
            return None;
        }
        let k = 96.0 / GetDpiForWindow(h).max(96) as f64;
        let l = |v: i32| (v as f64 * k * 10.0).round() / 10.0;
        Some((l(r.left), l(r.top), l(r.right - r.left), l(r.bottom - r.top)))
    }
}

fn remember(h: HWND, open: bool) {
    if crate::store::active() {
        let rect = logical_rect(h).or_else(|| SAVED.with(|s| s.get()));
        SAVED.with(|s| s.set(rect));
        crate::store::send(crate::store::Cmd::Window { name: "list".into(), open, rect });
    }
}

pub fn is_open() -> bool {
    !LIST.with(|l| l.get()).is_null()
}
/// Test helper: the list's v1.1 scroll bar window.
pub fn bar_for_test() -> HWND {
    unsafe { st(hwnd()).map_or(std::ptr::null_mut(), |s| s.vbar) }
}

pub fn hwnd() -> HWND {
    LIST.with(|l| l.get())
}

#[derive(Clone)]
pub struct Item {
    pub id: u64,
    pub title: String,
    pub preview: String,
    pub color: usize,
    pub open: bool,
    pub locked: bool,
    pub updated_at: u64,
}

struct State {
    search: HWND,
    rows: HWND,
    vbar: HWND,
    hot: Option<usize>,
    down: Option<usize>,
    ctx: Option<u64>,
}

struct Rows {
    items: Vec<Item>,
    scroll: i32,
    hot: Option<usize>,
    focus: Option<usize>,
    keyboard: bool,
    total: usize,
}

fn register() {
    unsafe {
        for (name, p) in [(CLASS, list_proc as unsafe extern "system" fn(_, _, _, _) -> _), (ROWS, rows_proc as _)] {
            let n = wide(name);
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: if name == ROWS { CS_DBLCLKS } else { 0 },
                lpfnWndProc: Some(p),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(std::ptr::null()),
                hIcon: if name == ROWS { std::ptr::null_mut() } else { crate::shell::app_icon(GetSystemMetrics(SM_CXICON)) },
                hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: n.as_ptr(),
                hIconSm: if name == ROWS { std::ptr::null_mut() } else { crate::shell::app_icon(GetSystemMetrics(SM_CXSMICON)) },
            };
            RegisterClassExW(&wc);
        }
    }
}

/// "방금 / n분 전 / n시간 전 / 어제 / n일 전 / M월 D일" (list.js ago()).
pub fn ago(ms: u64) -> String {
    if ms == 0 {
        return String::new();
    }
    let s = app::now_ms().saturating_sub(ms) as f64 / 1000.0;
    if s < 60.0 {
        "방금".into()
    } else if s < 3600.0 {
        format!("{}분 전", (s / 60.0) as u64)
    } else if s < 86400.0 {
        format!("{}시간 전", (s / 3600.0) as u64)
    } else if s < 172800.0 {
        "어제".into()
    } else if s < 604800.0 {
        format!("{}일 전", (s / 86400.0) as u64)
    } else {
        // local date without chrono: SYSTEMTIME of the timestamp
        unsafe {
            use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
            use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
            let t = ms * 10_000 + 116_444_736_000_000_000;
            let ft = FILETIME { dwLowDateTime: t as u32, dwHighDateTime: (t >> 32) as u32 };
            let mut utc: SYSTEMTIME = std::mem::zeroed();
            let mut loc: SYSTEMTIME = std::mem::zeroed();
            FileTimeToSystemTime(&ft, &mut utc);
            SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut loc);
            format!("{}월 {}일", loc.wMonth, loc.wDay)
        }
    }
}

/// Newest edit first; ties keep "newest created first"; filtered by the search box.
pub fn items(query: &str) -> (Vec<Item>, usize) {
    let q = query.trim().to_lowercase();
    app::with(|a| {
        let mut v: Vec<&app::Note> = a.notes.iter().collect();
        v.sort_by(|x, y| y.updated_at.cmp(&x.updated_at).then(y.seq.cmp(&x.seq)));
        let total = v.len();
        let items = v
            .into_iter()
            .filter(|n| q.is_empty() || format!("{}\n{}", n.title, n.text).to_lowercase().contains(&q))
            .map(|n| Item {
                id: n.id,
                title: n.title.clone(),
                preview: n.text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string(),
                color: n.color,
                open: n.open,
                locked: n.locked,
                updated_at: n.updated_at,
            })
            .collect();
        (items, total)
    })
}

unsafe fn st<'a>(h: HWND) -> Option<&'a mut State> {
    (GetWindowLongPtrW(h, GWLP_USERDATA) as *mut State).as_mut()
}
unsafe fn rs<'a>(h: HWND) -> Option<&'a mut Rows> {
    (GetWindowLongPtrW(h, GWLP_USERDATA) as *mut Rows).as_mut()
}

pub fn open(focus: bool) {
    let focus = focus && !crate::quiet();
    let h = hwnd();
    if !h.is_null() {
        unsafe {
            ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOWNA });
            if focus {
                SetForegroundWindow(h);
            }
        }
        return;
    }
    register();
    unsafe {
        let dpi = GetDpiForSystem();
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(mon, &mut mi);
        let (w, hh) = (px(g::LIST_W, dpi), px(g::LIST_H, dpi));
        let mut x = mi.rcWork.right - w - px(24.0, dpi);
        let mut y = mi.rcWork.top + px(72.0, dpi);
        let (mut w, mut hh) = (w, hh);
        // the saved place, if a monitor still shows its top (F-27)
        if let Some((lx, ly, lw, lh)) = SAVED.with(|s| s.get()) {
            let k = dpi as f64 / 96.0;
            let (sx, sy, sw, sh) = ((lx * k) as i32, (ly * k) as i32, (lw * k) as i32, (lh * k) as i32);
            let top = rect(sx, sy, sx + sw, sy + px(g::LIST_BAR, dpi));
            if sw > 0 && sh > 0 && !MonitorFromRect(&top, MONITOR_DEFAULTTONULL).is_null() {
                (x, y, w, hh) = (sx, sy, sw.max(px(g::LIST_MIN_W, dpi)), sh.max(px(g::LIST_MIN_H, dpi)));
            }
        }
        // test / measurement modes: created off-screen, so nothing ever flashes on the user's desktop
        let (x, y) = if crate::cfg().onscreen { (x, y) } else { (-15000, -15000) };
        let s = Box::new(State { search: std::ptr::null_mut(), rows: std::ptr::null_mut(), vbar: std::ptr::null_mut(), hot: None, down: None, ctx: None });
        // a taskbar button of its own (user decision 2026-11-16 — v1.1 kept the list off the
        // taskbar); WS_MINIMIZEBOX so its button minimises and restores it like a note's
        let ex = if crate::cfg().taskbar { WS_EX_APPWINDOW } else { WS_EX_TOOLWINDOW };
        let h = CreateWindowExW(
            ex,
            wide(CLASS).as_ptr(),
            wide("메모 목록").as_ptr(),
            WS_POPUP | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN,
            x,
            y,
            w,
            hh,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            GetModuleHandleW(std::ptr::null()),
            Box::into_raw(s) as *const _,
        );
        LIST.with(|l| l.set(h));
        ShowWindow(h, if focus { SW_SHOW } else { SW_SHOWNOACTIVATE });
        remember(h, true);
        if focus {
            SetForegroundWindow(h);
            if let Some(s) = st(h) {
                SetFocus(s.search); // list.js: search.focus()
            }
        }
    }
}

pub fn close() {
    let h = hwnd();
    if !h.is_null() {
        unsafe { DestroyWindow(h) };
    }
}

pub fn refresh() {
    let h = hwnd();
    if h.is_null() {
        return;
    }
    unsafe {
        if let Some(s) = st(h) {
            rebuild(s.rows, s.search);
        }
        InvalidateRect(h, std::ptr::null(), 0);
    }
}

/// Test / screenshot hooks.
pub fn set_query(q: &str) {
    let h = hwnd();
    unsafe {
        if let Some(s) = st(h) {
            SetWindowTextW(s.search, wide(q).as_ptr());
            rebuild(s.rows, s.search);
        }
    }
}

/// Opens the row context menu as a right-click at list-client (x, y) would; returns the menu.
pub fn context_at(x: i32, y: i32) -> HWND {
    let h = hwnd();
    unsafe {
        let Some(s) = st(h) else { return std::ptr::null_mut() };
        let mut p = POINT { x, y };
        ClientToScreen(h, &mut p);
        let mut q = p;
        ScreenToClient(s.rows, &mut q);
        let Some(i) = row_at(s.rows, q.y) else { return std::ptr::null_mut() };
        let it = rs(s.rows).unwrap().items[i].clone();
        s.ctx = Some(it.id);
        InvalidateRect(s.rows, std::ptr::null(), 0);
        menu::show_list_menu(h, it.locked, p)
    }
}

pub fn refresh_one(_id: u64) {
    refresh();
}

pub fn apply_theme() {
    let h = hwnd();
    if h.is_null() {
        return;
    }
    unsafe {
        relayout(h);
        if let Some(s) = st(h) {
            InvalidateRect(s.vbar, std::ptr::null(), 0);
        }
        RedrawWindow(h, std::ptr::null(), std::ptr::null_mut(), RDW_INVALIDATE | RDW_ALLCHILDREN);
    }
}

fn query(search: HWND) -> String {
    String::from_utf16_lossy(&crate::body::plain(search))
}

fn rebuild(rows: HWND, search: HWND) {
    unsafe {
        if let Some(r) = rs(rows) {
            let focused_id = r.focus.and_then(|i| r.items.get(i)).map(|it| it.id);
            let (items, total) = items(&query(search));
            r.focus = focused_id.and_then(|id| items.iter().position(|it| it.id == id)).or(r.focus.map(|i| i.min(items.len().saturating_sub(1)))).filter(|_| !items.is_empty());
            r.items = items;
            r.total = total;
            r.hot = None;
            update_scroll(rows);
            InvalidateRect(rows, std::ptr::null(), 0);
        }
    }
}

// ---------------------------------------------------------------- list window layout

struct Lay {
    band: i32,
    bar: RECT,
    tools: [RECT; 2],
    search_box: RECT,
    rows: RECT,
}

fn lay(h: HWND) -> Lay {
    let dpi = unsafe { GetDpiForWindow(h) };
    let mut rc = rect(0, 0, 0, 0);
    unsafe { GetClientRect(h, &mut rc) };
    let band = if app::band() == Band::None { 0 } else { px(4.0, dpi) };
    let bar = rect(0, band, rc.right, band + px(g::LIST_BAR, dpi));
    let t = px(g::TOOL, dpi);
    let ty = bar.top + (bar.bottom - bar.top - t) / 2;
    let right = rc.right - px(6.0, dpi);
    let close = rect(right - t, ty, right, ty + t);
    let new = rect(close.left - px(1.0, dpi) - t, ty, close.left - px(1.0, dpi), ty + t);
    let m = px(10.0, dpi);
    let search_box = rect(m, bar.bottom, rc.right - m, bar.bottom + px(g::SEARCH_H, dpi));
    let one = 1; // the window's black edge is one device pixel; rows sit just inside it
    let rows = rect(one, search_box.bottom + px(6.0, dpi), rc.right - one, rc.bottom - one);
    Lay { band, bar, tools: [new, close], search_box, rows }
}

fn relayout(h: HWND) {
    unsafe {
        let Some(s) = st(h) else { return };
        let dpi = GetDpiForWindow(h);
        let l = lay(h);
        let dc = GetDC(h);
        let f = gfx::font(FontSpec::px(130), dpi);
        let lh = gfx::line_h(dc, f);
        ReleaseDC(h, dc);
        let pad = px(10.0, dpi);
        let y = l.search_box.top + (l.search_box.bottom - l.search_box.top - lh) / 2;
        SetWindowPos(s.search, std::ptr::null_mut(), l.search_box.left + pad, y, l.search_box.right - l.search_box.left - 2 * pad, lh, SWP_NOZORDER | SWP_NOACTIVATE);
        SendMessageW(s.search, WM_SETFONT, f as usize, 1);
        update_scroll(s.rows);
    }
}

pub static PAINTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn paint(h: HWND) {
    PAINTS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dpi = unsafe { GetDpiForWindow(h) };
    let Some(s) = (unsafe { st(h) }) else { return };
    let p = pal(app::theme());
    let total = app::with(|a| a.notes.len());
    gfx::paint_buffered(h, |dc, w, hh| {
        let l = lay(h);
        gfx::fill(dc, &rect(0, 0, w, hh), p.paper);
        if l.band > 0 {
            // the six pastels side by side
            for k in 0..6 {
                gfx::fill(dc, &rect(w * k / 6, 0, w * (k + 1) / 6, l.band), PALETTE[k as usize]);
            }
        }
        let fh = gfx::font(FontSpec::bold(140), dpi);
        let x = px(14.0, dpi);
        let head = ws("메모");
        gfx::text(dc, &head, &rect(x, l.bar.top, w, l.bar.bottom), fh, p.ink, DT_SINGLELINE | DT_VCENTER);
        let hw = gfx::text_w(dc, &head, fh);
        gfx::text(dc, &ws(&total.to_string()), &rect(x + hw + px(6.0, dpi), l.bar.top, w, l.bar.bottom), gfx::font(FontSpec::px(140), dpi), p.muted, DT_SINGLELINE | DT_VCENTER);
        for (i, r) in l.tools.iter().enumerate() {
            let hot = s.hot == Some(i);
            if hot {
                gfx::fill(dc, r, if s.down == Some(i) { p.rule } else { p.paper3 });
            }
            icons::draw_centered(dc, if i == 0 { Icon::New } else { Icon::Close }, r, g::ICON, dpi, if hot { p.ink } else { p.muted }, false);
        }
        gfx::fill(dc, &l.search_box, p.paper2);
        gfx::frame(dc, &rect(0, 0, w, hh), EDGE);
    });
}

fn frame_px(dpi: u32) -> i32 {
    unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) }
}

fn hit(h: HWND, sx: i32, sy: i32) -> u32 {
    let mut p = POINT { x: sx, y: sy };
    unsafe { ScreenToClient(h, &mut p) };
    let dpi = unsafe { GetDpiForWindow(h) };
    let mut rc = rect(0, 0, 0, 0);
    unsafe { GetClientRect(h, &mut rc) };
    let b = frame_px(dpi);
    let (w, hh) = (rc.right, rc.bottom);
    if p.y < b {
        return if p.x < 2 * b {
            HTTOPLEFT
        } else if p.x >= w - 2 * b {
            HTTOPRIGHT
        } else {
            HTCAPTION
        };
    }
    // as in a note: 6 CSS px bands (also over the rows, which answer HTTRANSPARENT there),
    // 12 px corners, only the outer 3 px along a scroll bar
    let edge = px(6.0, dpi);
    let bar = unsafe { st(h).map_or(false, |s| !s.vbar.is_null() && IsWindowVisible(s.vbar) != 0) };
    let edge_right = if bar { px(3.0, dpi) } else { edge };
    if let Some(code) = crate::win::edge_hit(p.x, p.y, w, hh, edge, edge_right, px(12.0, dpi)) {
        return code;
    }
    let l = lay(h);
    if p.y < l.bar.bottom {
        if l.tools.iter().any(|r| p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom) {
            return HTCLIENT;
        }
        return HTCAPTION;
    }
    HTCLIENT
}

fn tool_at(h: HWND, x: i32, y: i32) -> Option<usize> {
    lay(h).tools.iter().position(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
}

unsafe extern "system" fn list_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(h, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(h, msg, wp, lp);
    }
    let Some(s) = st(h) else { return DefWindowProcW(h, msg, wp, lp) };
    match msg {
        WM_CREATE => {
            let pref: i32 = 1;
            DwmSetWindowAttribute(h, 33, &pref as *const i32 as *const _, 4);
            let hinst = GetModuleHandleW(std::ptr::null());
            s.search = CreateWindowExW(0, wide("EDIT").as_ptr(), std::ptr::null(), WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | ES_AUTOHSCROLL as u32, 0, 0, 10, 10, h, std::ptr::null_mut(), hinst, std::ptr::null());
            SendMessageW(s.search, EM_SETMARGINS, 3, 0);
            SetWindowSubclass(s.search, Some(search_proc), 2, 0);
            let r = Box::new(Rows { items: Vec::new(), scroll: 0, hot: None, focus: None, keyboard: false, total: 0 });
            s.rows = CreateWindowExW(0, wide(ROWS).as_ptr(), std::ptr::null(), WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS, 0, 0, 10, 10, h, std::ptr::null_mut(), hinst, Box::into_raw(r) as *const _);
            s.vbar = crate::sbar::create(h, crate::sbar::Target::Rows(s.rows));
            relayout(h);
            rebuild(s.rows, s.search);
            SetTimer(h, TIMER_AGO, 60_000, None); // keep "n분 전" fresh while the list is open
            0
        }
        WM_NCCALCSIZE => 0, // client = whole window (also the wp = 0 form from CreateWindowEx)
        // No non-client area (WM_NCCALCSIZE = 0), but on activation changes DefWindowProc
        // still paints the classic WS_THICKFRAME border over the client edges
        // (gray bevel on the left, right and bottom). lParam -1 = update state, don't draw.
        WM_NCACTIVATE => DefWindowProcW(h, WM_NCACTIVATE, wp, -1),
        WM_NCPAINT => 0,
        WM_NCHITTEST => hit(h, x_of(lp), y_of(lp)) as LRESULT,
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp as *mut MINMAXINFO);
            let dpi = GetDpiForWindow(h);
            mmi.ptMinTrackSize = POINT { x: px(g::LIST_MIN_W, dpi), y: px(g::LIST_MIN_H, dpi) };
            0
        }
        WM_SIZE => {
            if wp as u32 != SIZE_MINIMIZED {
                relayout(h);
                InvalidateRect(h, std::ptr::null(), 0);
            }
            0
        }
        WM_DPICHANGED => {
            let r = &*(lp as *const RECT);
            SetWindowPos(h, std::ptr::null_mut(), r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            relayout(h);
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint(h);
            0
        }
        WM_PRINTCLIENT => {
            gfx::print_into(wp as HDC, || paint(h));
            0
        }
        WM_MOUSEMOVE => {
            note::track(h, false);
            let t = tool_at(h, x_of(lp), y_of(lp));
            if t != s.hot {
                s.hot = t;
                InvalidateRect(h, std::ptr::null(), 0);
            }
            0
        }
        0x02A3 => {
            s.hot = None;
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            if let Some(t) = tool_at(h, x_of(lp), y_of(lp)) {
                s.down = Some(t);
                SetCapture(h);
                InvalidateRect(h, std::ptr::null(), 0);
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(t) = s.down.take() {
                ReleaseCapture();
                InvalidateRect(h, std::ptr::null(), 0);
                if tool_at(h, x_of(lp), y_of(lp)) == Some(t) {
                    if t == 0 {
                        app::new_note(None);
                    } else {
                        PostMessageW(h, WM_CLOSE, 0, 0);
                    }
                }
            }
            0
        }
        WM_COMMAND => {
            if lp as HWND == s.search && ((wp >> 16) & 0xFFFF) as u32 == EN_CHANGE {
                InvalidateRect(s.search, std::ptr::null(), 1); // placeholder on/off
                rebuild(s.rows, s.search);
            }
            0
        }
        WM_CTLCOLOREDIT => {
            let p = pal(app::theme());
            SetTextColor(wp as HDC, p.ink);
            SetBkColor(wp as HDC, p.paper2);
            gfx::brush(p.paper2) as LRESULT
        }
        WM_TIMER if wp == TIMER_AGO => {
            InvalidateRect(s.rows, std::ptr::null(), 0);
            0
        }
        WM_APP_MENU => {
            if Act::from_code(wp) == Act::ListDelete {
                if let Some(id) = s.ctx.take() {
                    note::request_delete(h, id);
                }
            }
            0
        }
        WM_APP_MENUCLOSED => {
            s.ctx = None;
            if let Some(r) = rs(s.rows) {
                let _ = r;
            }
            InvalidateRect(s.rows, std::ptr::null(), 0);
            0
        }
        WM_APP_CONFIRM => {
            if wp == 1 {
                app::delete_note(lp as u64);
            }
            SetFocus(s.rows);
            0
        }
        WM_CLOSE => {
            DestroyWindow(h);
            0
        }
        WM_EXITSIZEMOVE => {
            remember(h, true);
            0
        }
        WM_DESTROY => {
            remember(h, false);
            KillTimer(h, TIMER_AGO);
            LIST.with(|l| l.set(std::ptr::null_mut()));
            if crate::cfg().quit_when_empty && app::open_windows().is_empty() {
                PostQuitMessage(0);
            }
            0
        }
        WM_NCDESTROY => {
            let p = SetWindowLongPtrW(h, GWLP_USERDATA, 0) as *mut State;
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}

unsafe extern "system" fn search_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _i: usize, _d: usize) -> LRESULT {
    let list = GetParent(h);
    match msg {
        WM_KEYDOWN => match wp as u16 {
            VK_DOWN => {
                if let Some(s) = st(list) {
                    if let Some(r) = rs(s.rows) {
                        if !r.items.is_empty() {
                            r.focus = Some(0);
                            r.keyboard = true;
                        }
                    }
                    SetFocus(s.rows);
                }
                return 0;
            }
            VK_RETURN => {
                if let Some(s) = st(list) {
                    if let Some(id) = rs(s.rows).and_then(|r| r.items.first().map(|i| i.id)) {
                        app::open_note(id, None, true);
                    }
                }
                return 0;
            }
            VK_ESCAPE => {
                if GetWindowTextLengthW(h) > 0 {
                    SetWindowTextW(h, wide("").as_ptr());
                }
                return 0;
            }
            _ => {}
        },
        WM_CHAR if wp == 0x0D || wp == 0x1B => return 0,
        // "검색" placeholder in --color-muted, shown while empty even with the caret in it
        // (::placeholder; the system cue banner is COLOR_GRAYTEXT and hides on focus)
        WM_PAINT | WM_PRINTCLIENT => {
            let r = DefSubclassProc(h, msg, wp, lp);
            if GetWindowTextLengthW(h) == 0 {
                let dc = if msg == WM_PAINT { GetDC(h) } else { wp as HDC };
                let mut rc = rect(0, 0, 0, 0);
                GetClientRect(h, &mut rc);
                let f = SendMessageW(h, WM_GETFONT, 0, 0) as HFONT;
                gfx::text(dc, &ws("검색"), &rc, f, pal(app::theme()).muted, DT_SINGLELINE | DT_VCENTER | DT_LEFT);
                if msg == WM_PAINT {
                    ReleaseDC(h, dc);
                }
            }
            return r;
        }
        WM_NCDESTROY => {
            RemoveWindowSubclass(h, Some(search_proc), 2);
        }
        _ => {}
    }
    DefSubclassProc(h, msg, wp, lp)
}

// ---------------------------------------------------------------- rows

struct RowMetrics {
    pad_y: i32,
    /// top + bottom padding together (7 + 7 CSS px; 10.5 + 10.5 at 150 % is 21, not 2 × 11)
    pad_y2: i32,
    pad_x: i32,
    title_line: i32,
    prev_line: i32,
}

fn metrics(dpi: u32) -> RowMetrics {
    let f = |v: f32| (v * dpi as f32 / 96.0).round() as i32;
    RowMetrics { pad_y: px(7.0, dpi), pad_y2: px(14.0, dpi), pad_x: px(14.0, dpi) - 1 /* rows child starts 1 px in */, title_line: f(13.5 * 1.55), prev_line: f(12.5 * 1.55) }
}

fn row_h(it: &Item, m: &RowMetrics) -> i32 {
    m.pad_y2 + m.title_line + if it.preview.is_empty() { 0 } else { m.prev_line }
}

fn content_h(r: &Rows, dpi: u32) -> i32 {
    let m = metrics(dpi);
    if r.items.is_empty() {
        return 0;
    }
    r.items.iter().map(|it| row_h(it, &m)).sum::<i32>() + px(8.0, dpi)
}

fn update_scroll(h: HWND) {
    unsafe {
        let Some(r) = rs(h) else { return };
        let dpi = GetDpiForWindow(h);
        let mut rc = rect(0, 0, 0, 0);
        GetClientRect(h, &mut rc);
        let total = content_h(r, dpi);
        r.scroll = r.scroll.clamp(0, (total - rc.bottom).max(0));
        // v1.1 scroll bar (8 px, takes its width from the rows only while there is something
        // to scroll); the rows are placed here, from the list's layout
        let list = GetParent(h);
        let Some(s) = st(list) else { return };
        let l = lay(list);
        let sb = px(crate::sbar::WIDTH, dpi);
        let area_h = (l.rows.bottom - l.rows.top).max(1);
        let need = total > area_h;
        let w = l.rows.right - l.rows.left - if need { sb } else { 0 };
        let mut cur = rect(0, 0, 0, 0);
        GetWindowRect(h, &mut cur);
        if cur.right - cur.left != w || cur.bottom - cur.top != area_h {
            SetWindowPos(h, std::ptr::null_mut(), l.rows.left, l.rows.top, w, area_h, SWP_NOZORDER | SWP_NOACTIVATE);
            return; // WM_SIZE calls this again at the new size
        }
        // as in the note: from `sb` before the window edge, the last pixel under the border
        SetWindowPos(s.vbar, std::ptr::null_mut(), l.rows.right + 1 - sb, l.rows.top, sb - 1, area_h, SWP_NOZORDER | SWP_NOACTIVATE);
        crate::sbar::set(s.vbar, total, area_h, r.scroll);
    }
}

fn row_at(h: HWND, y: i32) -> Option<usize> {
    let r = unsafe { rs(h)? };
    let m = metrics(unsafe { GetDpiForWindow(h) });
    let mut top = -r.scroll;
    for (i, it) in r.items.iter().enumerate() {
        let hh = row_h(it, &m);
        if y >= top && y < top + hh {
            return Some(i);
        }
        top += hh;
    }
    None
}

fn ensure_visible(h: HWND, i: usize) {
    unsafe {
        let Some(r) = rs(h) else { return };
        let m = metrics(GetDpiForWindow(h));
        let top: i32 = r.items.iter().take(i).map(|it| row_h(it, &m)).sum();
        let hh = r.items.get(i).map(|it| row_h(it, &m)).unwrap_or(0);
        let mut rc = rect(0, 0, 0, 0);
        GetClientRect(h, &mut rc);
        if top < r.scroll {
            r.scroll = top;
        } else if top + hh > r.scroll + rc.bottom {
            r.scroll = top + hh - rc.bottom;
        }
        update_scroll(h);
    }
}

fn paint_rows(h: HWND) {
    let Some(r) = (unsafe { rs(h) }) else { return };
    let dpi = unsafe { GetDpiForWindow(h) };
    let p = pal(app::theme());
    let ctx = unsafe { st(GetParent(h)).and_then(|s| s.ctx) };
    let focused = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetFocus() == h };
    let m = metrics(dpi);
    gfx::paint_buffered(h, |dc, w, hh| unsafe {
        gfx::fill(dc, &rect(0, 0, w, hh), p.paper);
        if r.items.is_empty() {
            let msg = if r.total == 0 { "메모가 없어요" } else { "찾는 메모가 없어요" };
            let y = px(28.0, dpi);
            gfx::text(dc, &ws(msg), &rect(0, y, w, y + px(20.0, dpi)), gfx::font(FontSpec::px(130), dpi), p.muted, DT_SINGLELINE | DT_CENTER | DT_VCENTER);
            return;
        }
        let ft_open = gfx::font(FontSpec::bold(135), dpi);
        let ft_closed = gfx::font(FontSpec::px(135), dpi);
        let fw = gfx::font(FontSpec::px(115), dpi);
        let fp = gfx::font(FontSpec::px(125), dpi);
        let mut tm: TEXTMETRICW = std::mem::zeroed();
        let mut top = -r.scroll;
        for (i, it) in r.items.iter().enumerate() {
            let rh = row_h(it, &m);
            if top + rh > 0 && top < hh {
                let row = rect(0, top, w, top + rh);
                let is_ctx = ctx == Some(it.id);
                if is_ctx {
                    gfx::fill(dc, &row, p.paper3);
                } else if r.hot == Some(i) || (focused && r.focus == Some(i)) {
                    gfx::fill(dc, &row, p.paper2);
                }
                let y0 = top + m.pad_y;
                let dot = px(8.0, dpi);
                let dc_col = if it.open { PALETTE[it.color] } else { mix(PALETTE[it.color], p.paper, 0.4) };
                gfx::fill(dc, &rect(m.pad_x, y0 + px(6.0, dpi), m.pad_x + dot, y0 + px(6.0, dpi) + dot), dc_col);
                let tx = m.pad_x + dot + px(10.0, dpi);
                let when = ws(&ago(it.updated_at));
                let ww = gfx::text_w(dc, &when, fw);
                let wx = w - m.pad_x - ww;
                let ft = if it.open { ft_open } else { ft_closed };
                // baseline alignment of title and time (grid align-items: baseline)
                let old = SelectObject(dc, ft);
                GetTextMetricsW(dc, &mut tm);
                let (t_asc, t_h) = (tm.tmAscent, tm.tmHeight);
                SelectObject(dc, fw);
                GetTextMetricsW(dc, &mut tm);
                let w_asc = tm.tmAscent;
                SelectObject(dc, old);
                let baseline = y0 + (m.title_line - t_h) / 2 + t_asc;
                let untitled = it.title.trim().is_empty();
                let title_c = if !it.open || untitled { p.muted } else { p.ink };
                let mut x = tx;
                if it.locked {
                    let isz = px(12.0, dpi);
                    icons::draw(dc, Icon::Lock, x, baseline - isz + px(1.0, dpi), 12.0, dpi, title_c, false);
                    x += isz + px(4.0, dpi);
                }
                let title = if untitled { ws("제목 없음") } else { ws(it.title.trim()) };
                gfx::text(dc, &title, &rect(x, y0, wx - px(10.0, dpi), y0 + m.title_line), if untitled { ft_closed } else { ft }, title_c, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
                gfx::text(dc, &when, &rect(wx, baseline - w_asc, w - m.pad_x, baseline - w_asc + m.title_line), fw, p.muted, DT_SINGLELINE | DT_TOP | DT_LEFT);
                if !it.preview.is_empty() {
                    let py = y0 + m.title_line;
                    gfx::text(dc, &ws(&it.preview), &rect(tx, py, w - m.pad_x, py + m.prev_line), fp, p.muted, DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS);
                }
                if focused && r.keyboard && r.focus == Some(i) {
                    let t = px(1.5, dpi).max(1);
                    for k in 0..t {
                        gfx::frame(dc, &rect(row.left + k, row.top + k, row.right - k, row.bottom - k), p.ink);
                    }
                }
            }
            top += rh;
        }
    });
}

unsafe extern "system" fn rows_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let cs = &*(lp as *const CREATESTRUCTW);
        SetWindowLongPtrW(h, GWLP_USERDATA, cs.lpCreateParams as isize);
        return DefWindowProcW(h, msg, wp, lp);
    }
    let Some(r) = rs(h) else { return DefWindowProcW(h, msg, wp, lp) };
    let list = GetParent(h);
    match msg {
        WM_ERASEBKGND => 1,
        WM_NCHITTEST if crate::win::on_parent_edge(h, lp) => HTTRANSPARENT as LRESULT,
        WM_PAINT => {
            paint_rows(h);
            0
        }
        WM_PRINTCLIENT => {
            gfx::print_into(wp as HDC, || paint_rows(h));
            0
        }
        WM_SIZE => {
            update_scroll(h);
            0
        }
        WM_MOUSEMOVE => {
            note::track(h, false);
            let hot = row_at(h, y_of(lp));
            if hot != r.hot {
                r.hot = hot;
                InvalidateRect(h, std::ptr::null(), 0);
            }
            0
        }
        0x02A3 => {
            r.hot = None;
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_LBUTTONDOWN => {
            SetFocus(h);
            r.keyboard = false;
            0
        }
        WM_LBUTTONUP => {
            if let Some(i) = row_at(h, y_of(lp)) {
                r.focus = Some(i);
                let id = r.items[i].id;
                app::open_note(id, None, true);
            }
            0
        }
        WM_RBUTTONUP => {
            if let Some(i) = row_at(h, y_of(lp)) {
                let it = r.items[i].clone();
                if let Some(s) = st(list) {
                    s.ctx = Some(it.id);
                }
                InvalidateRect(h, std::ptr::null(), 0);
                let mut pt = POINT { x: x_of(lp), y: y_of(lp) };
                ClientToScreen(h, &mut pt);
                menu::show_list_menu(list, it.locked, pt);
            }
            0
        }
        WM_MOUSEWHEEL => {
            let delta = ((wp >> 16) & 0xFFFF) as u16 as i16 as i32;
            r.scroll -= delta * px(40.0, GetDpiForWindow(h)) / 120;
            update_scroll(h);
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        crate::sbar::WM_APP_SCROLLTO => {
            r.scroll = lp as i32;
            update_scroll(h);
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            if msg == WM_SETFOCUS && r.focus.is_none() && !r.items.is_empty() {
                r.focus = Some(0);
            }
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_GETDLGCODE => DLGC_WANTARROWS as LRESULT,
        WM_KEYDOWN => {
            let n = r.items.len();
            r.keyboard = true;
            match wp as u16 {
                VK_DOWN if n > 0 => {
                    let i = r.focus.map(|i| (i + 1).min(n - 1)).unwrap_or(0);
                    r.focus = Some(i);
                    ensure_visible(h, i);
                }
                VK_UP => match r.focus {
                    Some(0) | None => {
                        if let Some(s) = st(list) {
                            SetFocus(s.search);
                            SendMessageW(s.search, EM_SETSEL, 0, -1);
                        }
                    }
                    Some(i) => {
                        r.focus = Some(i - 1);
                        ensure_visible(h, i - 1);
                    }
                },
                VK_RETURN => {
                    if let Some(id) = r.focus.and_then(|i| r.items.get(i)).map(|it| it.id) {
                        app::open_note(id, None, true);
                    }
                }
                VK_DELETE => {
                    if let Some(id) = r.focus.and_then(|i| r.items.get(i)).map(|it| it.id) {
                        if !confirm::is_showing(list) {
                            note::request_delete(list, id);
                        }
                    }
                }
                _ => {}
            }
            InvalidateRect(h, std::ptr::null(), 0);
            0
        }
        WM_NCDESTROY => {
            let p = SetWindowLongPtrW(h, GWLP_USERDATA, 0) as *mut Rows;
            if !p.is_null() {
                drop(Box::from_raw(p));
            }
            DefWindowProcW(h, msg, wp, lp)
        }
        _ => DefWindowProcW(h, msg, wp, lp),
    }
}
